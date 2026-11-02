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
use crate::{exit_code_of, EXIT_OK};

/// The help text for the process subcommands.
pub const USAGE: &str = "\
Usage:
  o824 start  --install <id> [--port <n>] [--json]
  o824 stop   --install <id> [--json]
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
    let mut state = match o824_state::load(&state_path) {
        Ok(state) => state,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    // The concurrent-instance guard (§21.4): two forms sharing one home means two
    // runtimes writing the same sessions, and the second one is not a convenience.
    if let Some(existing) = state.processes.get(&install_id) {
        if is_alive(existing) {
            let error = Error::new(
                Code::Busy,
                format!(
                    "an instance of {install_id:?} is already running (pid {}); stop it first",
                    existing.pid
                ),
            )
            .with_param("pid", existing.pid);
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
        state.forget_process(&install_id);
    }

    let Some(package) = web_package(&context.home) else {
        let error = Error::new(
            Code::InstallDir,
            "o824 start: no global @deepseek-ai/dsh found; the web form is an npm package",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };

    let started = match o824_webform::start(
        &package,
        &context.home,
        &context.data_root,
        port,
        o824_webform::START_TIMEOUT,
    ) {
        Ok(started) => started,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    let mut record = started.record.clone();
    record.install_id = install_id.clone();
    state.remember_process(record.clone());
    if let Err(error) = o824_state::save(&state_path, &state) {
        // The process is running and unrecorded, which is worse than not starting it:
        // say so instead of leaving a server nobody can stop from this kernel.
        let _ = o824_webform::stop(&record);
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    if flags.flag("json") {
        let _ = writeln!(
            stdout,
            "{}",
            serde_json::json!({
                "installId": install_id,
                "pid": started.pid,
                "port": started.port,
                "url": started.url,
                "log": started.log_path.display().to_string(),
            })
        );
    } else {
        let _ = writeln!(
            stdout,
            "started {install_id} pid={} port={} ({})",
            started.pid, started.port, started.url
        );
        let _ = writeln!(stdout, "logs: {}", started.log_path.display());
    }
    EXIT_OK
}

/// `o824 stop`.
pub fn stop(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::valued("install"),
        FlagSpec::valued("home"),
        FlagSpec::valued("data-root"),
    ]);
    if let Err(error) = flags.parse("stop", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    let Some(context) = context_from(&mut flags) else {
        let error = Error::new(Code::Usage, "o824 stop: pass --root");
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };
    let install_id = flags.value("install").unwrap_or("web").to_owned();
    let state_path = context.data_root.join(o824_state::STATE_FILE_NAME);
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

    if let Err(error) = o824_webform::stop(&record) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    state.forget_process(&install_id);
    if let Err(error) = o824_state::save(&state_path, &state) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
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

/// `o824 status`.
pub fn status(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
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
        let rows: Vec<serde_json::Value> = state
            .processes
            .values()
            .map(|record| {
                serde_json::json!({
                    "installId": record.install_id,
                    "pid": record.pid,
                    "port": record.port,
                    "running": is_alive(record),
                    "log": record.log_path,
                })
            })
            .collect();
        let _ = writeln!(stdout, "{}", serde_json::Value::Array(rows));
        return EXIT_OK;
    }

    if state.processes.is_empty() {
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
    }
    EXIT_OK
}

/// `o824 logs`.
pub fn logs(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
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
    let text = match std::fs::read_to_string(&record.log_path) {
        Ok(text) => text,
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
    o824_webform::is_the_recorded_process(&record.image, record.created, record.pid)
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

/// The global web package, when this machine has one.
fn web_package(_home: &Path) -> Option<PathBuf> {
    for prefix in o824_discovery::global_prefixes() {
        if let Some(package) = o824_discovery::package_in_prefix(&prefix) {
            return Some(package);
        }
    }
    None
}

/// A start's timeout, re-exported so the CLI and the tests agree.
pub const TIMEOUT: Duration = o824_webform::START_TIMEOUT;
