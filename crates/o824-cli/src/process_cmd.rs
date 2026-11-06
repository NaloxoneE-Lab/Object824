//! `o824 start | stop | status | logs`: the web form as a process the kernel owns.
//!
//! The kernel starts a **detached** child, records exactly which process it was, and
//! later refuses to stop anything that is no longer that process. The record is the
//! only thing that makes "stop what you started" possible; a pid alone would not be
//! enough, because pids are reused.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use o824_errs::{Code, Error};
use o824_state::ProcessRecord;

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_OK, EXIT_STATE_DRIFT};

/// The help text for the process subcommands.
pub const USAGE: &str = "\
Usage:
  o824 start  --install <id> [--port <n>] [--json]
  o824 stop   --install <id> [--json]
  o824 stop   --all
  o824 status [--json]
  o824 logs   --install <id> [--tail <lines>]

`start` boots the web form of an installation as a detached process, waits until it
reports the port it listens on, and records what it started. `stop` stops exactly that
process, and refuses when the pid now belongs to something else.

Port 0 (the default) lets the operating system pick a free one. A second start against
a DSH home that already has a live process is refused: two forms sharing one home is
the concurrent-instance risk, not a convenience.

Exit code 0 means the command did what it says; 7 means another invocation holds the
lock; 5 means the recorded state no longer matches the machine.
";

/// What the subcommands need from the caller.
pub struct Context {
    /// The data root holding the state file.
    pub data_root: PathBuf,
    /// The DSH home the process would use.
    pub home: PathBuf,
}

/// `o824 start`.
pub fn start(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }
    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::valued("install"),
        FlagSpec::valued("port"),
        FlagSpec::valued("home"),
        FlagSpec::valued("data-root"),
    ]);
    if let Err(error) = flags.parse("start", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let Some(context) = context_from(&mut flags) else {
        let error = Error::new(
            Code::Usage,
            "o824 start: pass --root (or install an editable tree)",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };
    let port = match flags.value("port") {
        Some(value) => match value.trim().parse::<u16>() {
            Ok(port) => Some(port),
            Err(_) => {
                let error = Error::new(
                    Code::Usage,
                    format!("o824 start: --port expects a number, got {value:?}"),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        },
        None => None,
    };

    let install_id = flags.value("install").unwrap_or("web").to_owned();
    let state_path = context.data_root.join(o824_state::STATE_FILE_NAME);
    // The machine lock is apply's: it holds it across the whole start or stop, and a
    // second copy here would deadlock against it. The checks above are pre-checks for a
    // fast refusal; apply re-checks under the lock.
    let _ = &o824_lock::acquire;
    let mut state = match o824_state::load(&state_path) {
        Ok(state) => state,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    // The concurrent-instance guard (§21.4): two forms sharing one home means two
    // runtimes writing the same sessions, and the second one is not a convenience.
    // §21.4's first risk is about the *home*, not about the label: two forms (or two
    // starts under different labels) sharing one DSH home share its sessions.
    if let Some(existing) = blocking_record(&state, &context.home) {
        let error = Error::new(
            Code::ConcurrentInstance,
            format!(
                "an instance for this DSH home is already running ({} pid {}); stop it first",
                existing.install_id, existing.pid
            ),
        )
        .with_param("pid", existing.pid)
        .with_param("home", existing.home.clone());
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    state.processes.retain(|_, record| is_alive(record));

    // A port that is already being listened on is not a start failure to report
    // generically: naming the owner is what lets a person tell "this is my old server"
    // from "this is something I care about".
    if let Some(port) = port {
        if let Some(owner) = o824_formproc::port_owner(port) {
            let error = Error::new(
                Code::PortInUse,
                format!(
                    "port {port} is already listened on by pid {} ({})",
                    owner.pid,
                    if owner.image.is_empty() {
                        "unknown"
                    } else {
                        &owner.image
                    }
                ),
            )
            .with_param("port", port)
            .with_param("pid", owner.pid);
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    }

    // §21.4: the forms share one DSH home, so a desktop instance that is already
    // running means two runtimes on the same sessions. Warn rather than refuse — the
    // user may know exactly why they are starting a second one.
    if let Some(warning) = desktop_already_running(&context.home) {
        let _ = writeln!(stderr, "{warning}");
    }

    // §21.5: starting is a plan action, not an RPC of its own. The plan is built here and
    // run through apply, so the CLI and the protocol take one path; the guard above stays
    // as a pre-check so a refusal arrives before any work.
    // §21.5's rule for the desktop form: if one is running, report it; do not open a
    // second window on a profile DSH takes exclusively.
    if install_id == "desktop" || find_install(&install_id, "web").is_none() {
        if let Some(host) = o824_discovery::host_processes().first() {
            let already = Error::new(
                Code::ConcurrentInstance,
                "desktop: the app is already running",
            );
            let _ = already;
            if flags.flag("json") {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({
                        "installId": "desktop",
                        "pid": host.pid,
                        "port": serde_json::Value::Null,
                        "url": serde_json::Value::Null,
                        "alreadyRunning": true,
                    })
                );
            } else {
                let _ = writeln!(
                    stdout,
                    "desktop is already running (pid {}); nothing to start",
                    host.pid
                );
            }
            return EXIT_OK;
        }
    }

    let Some(install) = find_install(&install_id, "web") else {
        let error = Error::new(
            Code::InstallDir,
            "o824 start: no web installation found; run `o824 discover` to see what is installed",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };
    // The plan names the installation the way the user did ("web"), because that is the
    // key the record is kept under and the name every later command uses.
    let mut install = install;
    install.install_id = install_id.clone();
    let options = o824_plan::StartOptions {
        home: context.home.clone(),
        port,
        package_dir: Some(PathBuf::from(&install.root)),
    };
    let mut plan = match o824_plan::process::build_process(
        &install,
        o824_plan::ACTION_START,
        o824_model::Timestamp::now(),
        &context.data_root,
        &options,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    // The plan names the executable it will start; kept for the "someone else
    // started it" answer below, where there is no record to point at.
    let desktop_exe = plan
        .steps
        .iter()
        .find(|step| step.kind == o824_plan::KIND_START_DESKTOP)
        .map(|step| PathBuf::from(&step.path));
    let scope = PathBuf::from(&install.root);
    if let Err(error) = o824_apply::run(&context.data_root, &mut plan, &scope) {
        // A failed start leaves no record behind: apply removes the provisional one.
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let started = match o824_state::load(&state_path) {
        Ok(after) => after
            .processes
            .get(&install_id)
            .cloned()
            .or_else(|| blocking_record(&after, &context.home).cloned()),
        Err(_) => None,
    };
    let Some(started) = started else {
        // `apply` records nothing when it found the desktop already running: the process
        // exists but it is not ours. Report that one rather than failing — §2 of the
        // 2026-11-06 note: "report that pid and exit 0".
        if let Some(pid) = desktop_exe
            .as_deref()
            .and_then(o824_formproc::desktop::running_instance)
        {
            if flags.flag("json") {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({
                        "installId": install_id,
                        "pid": pid,
                        "port": serde_json::Value::Null,
                        "url": serde_json::Value::Null,
                        "alreadyRunning": true,
                    })
                );
            } else {
                let _ = writeln!(
                    stdout,
                    "o824 start: {} is already running (pid {pid}); nothing was started",
                    install_id
                );
            }
            return EXIT_OK;
        }
        let error = Error::new(
            Code::Internal,
            "o824 start: the plan ran but recorded no process",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };
    let _ = &mut state;
    let url = o824_formproc::url_from_log(std::path::Path::new(&started.log_path));
    let port_text = started
        .port
        .map(|port| port.to_string())
        .unwrap_or_else(|| "unknown".to_owned());

    if flags.flag("json") {
        let _ = writeln!(
            stdout,
            "{}",
            serde_json::json!({
                "installId": started.install_id,
                "pid": started.pid,
                "port": started.port,
                "url": url,
                "log": started.log_path,
            })
        );
    } else {
        let _ = writeln!(
            stdout,
            "started {} pid={} port={}{}",
            started.install_id,
            started.pid,
            port_text,
            url.as_ref()
                .map(|url| format!(" ({url})"))
                .unwrap_or_default()
        );
        let _ = writeln!(stdout, "logs: {}", started.log_path);
    }
    EXIT_OK
}

/// `o824 stop`.
pub fn stop(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }
    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::boolean("all"),
        FlagSpec::valued("install"),
        FlagSpec::valued("home"),
        FlagSpec::valued("data-root"),
    ]);
    if let Err(error) = flags.parse("stop", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    // `--all` stops everything this kernel started, which is what a person means after
    // a session went wrong. It needs no home: each record already carries its own.
    if flags.flag("all") {
        return stop_all(flags.value("data-root").map(PathBuf::from), stdout, stderr);
    }

    // `stop` needs only the data root: the record carries the home it was started with,
    // which is also why `--all` needs nothing else.
    let data_root = flags
        .value("data-root")
        .map(PathBuf::from)
        .unwrap_or_else(|| o824_state::data_root(None));
    let install_id = flags.value("install").unwrap_or("web").to_owned();
    let state_path = data_root.join(o824_state::STATE_FILE_NAME);
    // The lock belongs to apply, which holds it across the whole stop.
    let _ = &o824_lock::acquire;
    let mut state = match o824_state::load(&state_path) {
        Ok(state) => state,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    let Some(record) = state.processes.get(&install_id).cloned() else {
        if flags.flag("json") {
            let _ = writeln!(
                stdout,
                "{}",
                serde_json::json!({ "stopped": false, "reason": "not running" })
            );
        } else {
            let _ = writeln!(stdout, "{install_id} is not running");
        }
        return EXIT_OK;
    };

    // Two states look alike and mean opposite things. A pid with no process is the
    // normal state after a crash: the desired state is already reached, and the stale
    // record should go. A pid that now belongs to something else is the dangerous one:
    // the record is the only evidence we started something here, so it is kept and the
    // stop is refused.
    if o824_formproc::image_of(record.pid).is_none() {
        state.forget_process(&install_id);
        if let Err(error) = o824_state::save(&state_path, &state) {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
        if flags.flag("json") {
            let _ = writeln!(
                stdout,
                "{}",
                serde_json::json!({ "stopped": false, "reason": "already stopped", "pid": record.pid })
            );
        } else {
            let _ = writeln!(
                stdout,
                "{install_id} was already stopped (pid {})",
                record.pid
            );
        }
        return EXIT_OK;
    }

    // §21.5: stopping is a plan action. The plan names the installation, and apply
    // re-checks the identity of the process it is about to stop — which is the only check
    // that still matters by the time it runs.
    let install = find_install(&record.install_id, "web").unwrap_or_else(|| {
        // A record whose installation is gone can still be stopped: the plan needs the id
        // and the shape, and the record carries both.
        o824_model::Install {
            install_id: record.install_id.clone(),
            kind: if record.port.is_some() {
                "web"
            } else {
                "desktop"
            }
            .to_owned(),
            root: record.home.clone(),
            home: record.home.clone(),
            version: String::new(),
            profiles: Vec::new(),
            health: "ok".to_owned(),
        }
    });
    let mut install = install;
    install.install_id = record.install_id.clone();
    let mut plan = match o824_plan::process::build_process(
        &install,
        o824_plan::ACTION_STOP,
        o824_model::Timestamp::now(),
        &data_root,
        &o824_plan::StartOptions::default(),
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    // The plan names the executable it will start; kept for the "someone else started it"
    // answer below, where there is no record to point at.
    let scope = PathBuf::from(&install.root);
    if let Err(error) = o824_apply::run(&data_root, &mut plan, &scope) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    // The process is gone; the port may not be. A child of the server can hold it, and
    // saying so is more useful than letting the next start fail with "port in use".
    if let Some(port) = record.port {
        let _ = o824_formproc::wait_for_port_release(port, Duration::from_secs(5));
        if let Some(warning) = leftover_warning(&record) {
            let _ = writeln!(stderr, "{warning}");
        }
    }

    if flags.flag("json") {
        let _ = writeln!(
            stdout,
            "{}",
            serde_json::json!({ "stopped": true, "pid": record.pid })
        );
    } else {
        let _ = writeln!(stdout, "stopped {install_id} (pid {})", record.pid);
    }
    EXIT_OK
}

/// `o824 stop --all`: every process this kernel started.
fn stop_all(data_root: Option<PathBuf>, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let data_root = data_root.unwrap_or_else(|| o824_state::data_root(None));
    let state_path = data_root.join(o824_state::STATE_FILE_NAME);
    let mut state = match o824_state::load(&state_path) {
        Ok(state) => state,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    let mut stopped = 0usize;
    let mut refused = 0usize;
    let ids: Vec<String> = state.processes.keys().cloned().collect();
    for install_id in ids {
        let Some(record) = state.processes.get(&install_id).cloned() else {
            continue;
        };
        // A record whose process is already gone is not a failure: it is the common
        // case after a crash, and the record is simply stale.
        if !is_alive(&record) {
            state.forget_process(&install_id);
            continue;
        }
        match o824_formproc::stop(&record) {
            Ok(()) => {
                stopped += 1;
                let _ = writeln!(stdout, "stopped {install_id} (pid {})", record.pid);
                state.forget_process(&install_id);
            }
            Err(error) => {
                refused += 1;
                let _ = writeln!(stderr, "{error}");
            }
        }
    }

    if let Err(error) = o824_state::save(&state_path, &state) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if stopped == 0 && refused == 0 {
        let _ = writeln!(stdout, "nothing running");
    }
    if refused > 0 {
        // Something is still running and this kernel will not stop it. Reporting success
        // would be the one thing a cleanup command must not do.
        return EXIT_STATE_DRIFT;
    }
    EXIT_OK
}

/// `o824 status`.
pub fn status(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }
    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::valued("data-root"),
    ]);
    if let Err(error) = flags.parse("status", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    let data_root = match flags.value("data-root") {
        Some(root) => PathBuf::from(root),
        None => o824_state::data_root(None),
    };
    let state = match o824_state::load(&data_root.join(o824_state::STATE_FILE_NAME)) {
        Ok(state) => state,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    if flags.flag("json") {
        let mut rows: Vec<serde_json::Value> = state
            .processes
            .values()
            .map(|record| {
                serde_json::json!({
                    "installId": record.install_id,
                    "pid": record.pid,
                    "port": record.port,
                    "running": is_alive(record),
                    "startedBy": "kernel",
                    // The URL carries the trust token, so it is read from the log rather
                    // than stored — but a person asking what is running needs it.
                    "url": o824_formproc::url_from_log(std::path::Path::new(&record.log_path)),
                    "log": record.log_path,
                })
            })
            .collect();
        for host in foreign_hosts(&state) {
            rows.push(serde_json::json!({
                "installId": "desktop",
                "pid": host.pid,
                "port": serde_json::Value::Null,
                "running": true,
                "startedBy": "other",
                "log": serde_json::Value::Null,
            }));
        }
        let _ = writeln!(stdout, "{}", serde_json::Value::Array(rows));
        return EXIT_OK;
    }

    // §21.2's `status()` is "what is running", not "what I started": a desktop instance
    // the user launched themselves is exactly the thing the next start must know about.
    let foreign = foreign_hosts(&state);
    if state.processes.is_empty() && foreign.is_empty() {
        let _ = writeln!(stdout, "nothing started by this kernel");
        return EXIT_OK;
    }
    for record in state.processes.values() {
        let state_word = if is_alive(record) { "running" } else { "gone" };
        let port = record
            .port
            .map(|port| port.to_string())
            .unwrap_or_else(|| "-".to_owned());
        let _ = writeln!(
            stdout,
            "{:<8} {:<8} pid={:<7} port={:<6} {}",
            record.install_id, state_word, record.pid, port, record.log_path
        );
        if is_alive(record) {
            if let Some(url) = o824_formproc::url_from_log(std::path::Path::new(&record.log_path)) {
                let _ = writeln!(stdout, "         {url}");
            }
        }
        if !is_alive(record) {
            if let Some(warning) = leftover_warning(record) {
                let _ = writeln!(stdout, "         {warning}");
            }
        }
    }
    for host in foreign {
        let _ = writeln!(
            stdout,
            "{:<8} {:<8} pid={:<7} port={:<6} started outside this kernel",
            "desktop", "running", host.pid, "-"
        );
    }
    EXIT_OK
}

/// The live record, if any, that shares `home`.
///
/// §21.4's first risk is about the *home*, not about a label: two forms — or two starts
/// under different labels — sharing one DSH home share its sessions.
pub fn blocking_record<'a>(
    state: &'a o824_state::State,
    home: &Path,
) -> Option<&'a o824_state::ProcessRecord> {
    let home = home.display().to_string();
    state
        .processes
        .values()
        .find(|record| same_home(&record.home, &home) && is_alive(record))
}

/// True when two paths name the same directory, ignoring case and spelling.
fn same_home(left: &str, right: &str) -> bool {
    o824_model::path::clean(Path::new(left))
        .display()
        .to_string()
        .eq_ignore_ascii_case(
            &o824_model::path::clean(Path::new(right))
                .display()
                .to_string(),
        )
}

/// Running desktop instances that no record claims.
fn foreign_hosts(state: &o824_state::State) -> Vec<o824_discovery::RunningHost> {
    let recorded: Vec<u32> = state.processes.values().map(|record| record.pid).collect();
    o824_discovery::host_processes()
        .into_iter()
        .filter(|host| !recorded.contains(&host.pid))
        .collect()
}

/// `o824 logs`.
pub fn logs(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }
    let mut flags = FlagSet::new(vec![
        FlagSpec::valued("install"),
        FlagSpec::valued("tail"),
        FlagSpec::valued("data-root"),
    ]);
    if let Err(error) = flags.parse("logs", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    let data_root = match flags.value("data-root") {
        Some(root) => PathBuf::from(root),
        None => o824_state::data_root(None),
    };
    let install_id = flags.value("install").unwrap_or("web").to_owned();
    let tail: usize = flags
        .value("tail")
        .and_then(|value| value.parse().ok())
        .unwrap_or(40);

    let state = match o824_state::load(&data_root.join(o824_state::STATE_FILE_NAME)) {
        Ok(state) => state,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    let Some(record) = state.processes.get(&install_id) else {
        let error = Error::new(
            Code::Usage,
            format!("o824 logs: {install_id} was not started by this kernel"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };
    let text = match std::fs::read(&record.log_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) => {
            let error = Error::new(
                Code::Internal,
                format!("o824 logs: cannot read {}: {error}", record.log_path),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(tail);
    for line in &lines[start..] {
        let _ = writeln!(stdout, "{line}");
    }
    EXIT_OK
}

/// True when the recorded process is still the process it recorded.
pub fn is_alive(record: &ProcessRecord) -> bool {
    o824_formproc::is_the_recorded_process(&record.image, record.created, record.pid)
}

/// The data root and home a subcommand works against.
fn context_from(flags: &mut FlagSet) -> Option<Context> {
    let home = flags.value("home").map(PathBuf::from)?;
    let data_root = match flags.value("data-root") {
        Some(root) => PathBuf::from(root),
        None => o824_state::data_root(None),
    };
    Some(Context { data_root, home })
}

/// The warning for a process that is gone while its port is still held.
///
/// This is §21.4's third risk in its actionable form: the server was stopped (or died),
/// and something else — usually a child it spawned — still holds the port. Naming the
/// owner is the difference between "port busy" and a fact a person can act on.
pub fn leftover_warning(record: &ProcessRecord) -> Option<String> {
    let port = record.port?;
    let owner = o824_formproc::port_owner(port)?;
    if owner.pid == record.pid {
        // The process itself is still there; this is not a leftover.
        return None;
    }
    Some(format!(
        "warning: port {port} is still listened on by pid {} ({}); {} is not running",
        owner.pid,
        if owner.image.is_empty() {
            "unknown"
        } else {
            &owner.image
        },
        record.install_id
    ))
}

/// The warning shown when a desktop instance is already running.
///
/// The forms share one DSH home, so a second runtime sees the same sessions. The
/// kernel warns and lets the user decide: refusing would be wrong for someone who
/// knows why, and silence would be wrong for everyone else.
pub fn desktop_already_running(home: &Path) -> Option<String> {
    // Only the default home is shared with a desktop instance the user launched: a start
    // against an explicit temporary home shares nothing with it.
    let default_home = o824_discovery::resolve_home(None);
    if !same_home(
        &home.display().to_string(),
        &default_home.display().to_string(),
    ) {
        return None;
    }
    let hosts = o824_discovery::host_processes();
    let first = hosts.first()?;
    let count = if hosts.len() == 1 {
        String::new()
    } else {
        format!(" and {} more", hosts.len() - 1)
    };
    Some(format!(
        "warning: {} is already running (pid {}{count}); it shares this DSH home, so both runtimes will see the same sessions",
        o824_discovery::EXE_NAME,
        first.pid
    ))
}

/// The installation a start should bring up.
///
/// `--install` is either an identifier `o824 discover` printed or a kind name; a kind is
/// what a person types when they do not care which of two copies runs.
fn find_install(install_id: &str, kind: &str) -> Option<o824_model::Install> {
    let installs = o824_discovery::discover(&o824_discovery::Options::default()).ok()?;
    installs
        .iter()
        .find(|install| install.install_id == install_id)
        .or_else(|| installs.iter().find(|install| install.kind == install_id))
        .or_else(|| installs.iter().find(|install| install.kind == kind))
        .cloned()
}

/// A start's timeout, re-exported so the CLI and the tests agree.
pub const TIMEOUT: Duration = o824_formproc::START_TIMEOUT;
