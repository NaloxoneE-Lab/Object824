//! The O824 window.
//!
//! §7.5: the interface is a **client** of the kernel, never its host. This binary spawns
//! `o824 kernel --stdio`, asks it questions, and draws the answers. It does no diagnosis
//! and no repair of its own — if the kernel is missing, the window says so and points at
//! the command line rather than trying to be clever.
//!
//! Two things here are not boilerplate:
//!
//! * **No administrator.** A window that needs elevation is a window most people cannot
//!   open twice. WebView2 keeps its data under `%LOCALAPPDATA%`; if a previous elevated
//!   run created that folder, a normal run inherits a folder it cannot write and the app
//!   dies before drawing anything. So the data directory is chosen here, created by this
//!   process with this user's rights, and a failure to start shows a native message box
//!   instead of a silent crash.
//! * **The copy comes from `o824-present`.** No sentence in this file is user-facing; the
//!   strings live in the copy crate where they are tested for jargon and for having a
//!   next step.

// One Win32 call (the message box) has no safe wrapper in the crates this uses; it is
// isolated in `report_start_failure` and touches two owned strings and no other pointers.
#![deny(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use o824_bridge::{Bridge, Options};
use o824_present::{first_use, Installation, Locale};
use serde::Serialize;

/// What the window asks for when it starts.
#[derive(Serialize)]
struct Screen {
    /// The sentence group for the first screen.
    headline: String,
    reason: String,
    next_step: String,
    /// The installations the kernel found, already described for a person.
    installations: Vec<InstallRow>,
}

/// One installation, as the window shows it.
#[derive(Serialize)]
struct InstallRow {
    id: String,
    name: String,
    version: String,
    findings: usize,
    repairable: usize,
}

/// The kernel this window talks to, kept for the life of the window.
struct Kernel(Mutex<Option<Bridge>>);

/// The last concrete reason the window could not start, kept so the dialog can say it.
///
/// "The window could not be created" without the reason is the kind of message that makes
/// someone open Task Manager; the directory and the OS error are what make it actionable.
static LAST_PROBLEM: Mutex<Option<String>> = Mutex::new(None);

fn remember_problem(text: String) {
    log_line(&format!("problem: {text}"));
    if let Ok(mut slot) = LAST_PROBLEM.lock() {
        *slot = Some(text);
    }
}

fn last_problem() -> String {
    LAST_PROBLEM
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| "no detail was recorded".to_owned())
}

/// Appends one line to `%LOCALAPPDATA%\O824\gui.log`.
///
/// A window that dies before it draws anything leaves no console and no event log entry a
/// person can find, so the first thing this program does is make sure the next crash is
/// readable. Every line is flushed: the process may not get to run a destructor.
fn log_line(line: &str) {
    use std::io::Write as _;
    // Candidates in order of how likely they are to be writable for the account that
    // double-clicked: next to the program (it lives in a folder the user owns),
    // %LOCALAPPDATA%, then %TEMP%. A file, not a directory — creating a file succeeds in
    // places where creating a folder is denied.
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(directory) = exe.parent() {
            candidates.push(directory.join("o824-gui.log"));
        }
    }
    for variable in ["LOCALAPPDATA", "TEMP"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(std::path::PathBuf::from(base).join("O824-gui.log"));
        }
    }
    for path in candidates {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let since_epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0);
            let _ = writeln!(file, "{since_epoch} pid={} {line}", std::process::id());
            let _ = file.flush();
            return;
        }
    }
}

fn main() {
    log_line("=== start ===");
    log_line("=== start ===");
    log_line(&format!(
        "exe={:?} cwd={:?}",
        std::env::current_exe().ok(),
        std::env::current_dir().ok()
    ));
    std::panic::set_hook(Box::new(|info| {
        log_line(&format!("PANIC: {info}"));
        // The call stack names the call that was denied; the message alone does not.
        log_line(&format!(
            "BACKTRACE:
{}",
            std::backtrace::Backtrace::force_capture()
        ));
    }));
    // Before Tauri starts: the data directory it will use has to be ours, not one an
    // elevated run left behind.
    prepare_webview_data();
    log_line(&format!(
        "kernel={:?} webview_data={:?}",
        resolve_kernel(),
        std::env::var_os("WEBVIEW2_USER_DATA_FOLDER")
    ));

    let data_directory = usable_data_directory();
    let started = std::panic::catch_unwind(|| {
        tauri::Builder::default()
        .setup(move |app| {
            // §7.5: the window is created here rather than by the config because its
            // browser data directory has to be a path this account can actually write.
            // `webview2_user_data_folder` is not consulted by the runtime — the directory
            // is passed to WebView2 explicitly, which is why it has to be set here.
            let url = tauri::WebviewUrl::App("index.html".into());
            let mut builder = tauri::WebviewWindowBuilder::new(app, "main", url)
                .title("O824")
                .inner_size(880.0, 640.0)
                .min_inner_size(640.0, 480.0)
                .center();
            if let Some(directory) = data_directory.clone() {
                builder = builder.data_directory(directory);
            }
            builder.build()?;
            Ok(())
        })
        .manage(Kernel(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![screen, diagnose])
        .run(tauri::generate_context!())
    });

    // §7.4: a window that cannot start must say where the command line is, because the
    // command line still works. Tauri reports a failed setup by panicking, so the catch is
    // around the call rather than on its result.
    match started {
        Ok(Ok(())) => log_line("=== window closed normally ==="),
        Ok(Err(error)) => {
            log_line(&format!("tauri error: {error}"));
            report_start_failure(&error.to_string());
        }
        Err(_) => {
            report_start_failure(&last_problem());
        }
    }
}

/// Points WebView2 at a directory this user owns, creating it if needed.
///
/// WebView2 defaults to `%LOCALAPPDATA%\<identifier>`. If that folder was created by an
/// elevated run it carries a high integrity level, and a normal run gets access denied
/// before the first pixel. Choosing the path here and creating it as this user removes the
/// whole class of failure; when even that fails, the app reports it instead of crashing.
/// The first directory WebView2 may use, chosen by trying it.
///
/// The candidates are the program's own folder first — it lives somewhere the account
/// owns — then `%LOCALAPPDATA%`, then `%TEMP%`. On a machine where the latter two refuse
/// new folders (this one does), the first still works.
fn usable_data_directory() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(directory) = exe.parent() {
            candidates.push(directory.join("O824-webview2"));
        }
    }
    for variable in ["LOCALAPPDATA", "TEMP"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(std::path::PathBuf::from(base).join("O824-webview2"));
        }
    }
    for directory in candidates {
        match std::fs::create_dir_all(&directory) {
            Ok(()) => {
                log_line(&format!("webview data dir ready: {}", directory.display()));
                return Some(directory);
            }
            Err(error) => log_line(&format!(
                "webview data dir FAILED: {} ({error})",
                directory.display()
            )),
        }
    }
    log_line("webview data dir: none usable, leaving the default");
    None
}

fn prepare_webview_data() {
    // More than one candidate, in order: a folder this account owns, then the temp
    // directory, then WebView2's own default. An intermediate directory created by another
    // account is what broke the first attempt, and a single hard-coded path is one more
    // thing that can be wrong on one machine.
    let mut candidates = Vec::new();
    for variable in ["LOCALAPPDATA", "TEMP"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(std::path::PathBuf::from(base).join("O824-webview2"));
        }
    }
    for directory in candidates {
        // Everything Windows can tell us about the target before we try: "access denied"
        // arrives for a file where a directory was expected, for a missing parent, and for a
        // genuinely forbidden path, and the three have different fixes.
        let parent = directory.parent().map(|path| path.to_path_buf());
        log_line(&format!(
            "data dir probe: target={} exists={:?} parent={:?} parent_exists={:?} parent_writable={:?}",
            directory.display(),
            std::fs::metadata(&directory).map(|meta| meta.is_dir()),
            parent.as_ref().map(|path| path.display().to_string()),
            parent.as_ref().map(|path| path.exists()),
            parent.as_ref().map(|path| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path.join("o824-write-probe.tmp"))
                    .map(|_| true)
                    .unwrap_or(false)
            })
        ));
        match std::fs::create_dir_all(&directory) {
            Ok(()) => {
                log_line(&format!("webview data dir ready: {}", directory.display()));
                std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &directory);
                return;
            }
            Err(error) => remember_problem(format!(
                "cannot use {} as the browser data folder ({error})",
                directory.display()
            )),
        }
    }
    log_line("webview data dir: none usable, leaving the default");
}

/// The first screen: what is installed, and what the main button should say.
///
/// `async` on purpose: Tauri runs a synchronous command on the main thread, and this one
/// spawns a process and waits for a diagnosis — a window that freezes while it thinks is
/// indistinguishable from a window that has crashed.
#[tauri::command]
async fn screen(kernel: tauri::State<'_, Kernel>) -> Result<Screen, String> {
    let locale = Locale::from_system(std::env::var("LANG").ok().as_deref());
    let mut guard = kernel.0.lock().map_err(|_| "the kernel handle is poisoned")?;
    let bridge = match guard.as_mut() {
        Some(bridge) => bridge,
        None => {
            let started = Bridge::start(&Options {
                program: resolve_kernel(),
                ..Options::default()
            })
            .map_err(|error| error.to_string())?;
            guard.insert(started)
        }
    };

    let installs = bridge.discover().map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for install in installs {
        // The diagnosis is one call per installation; its findings count is what the first
        // screen needs, and the full list comes from `diagnose`.
        let findings = bridge.doctor(&install.install_id).map_err(|error| error.to_string())?;
        let count = findings
            .get("findings")
            .and_then(|value| value.as_array())
            .map(Vec::len)
            .unwrap_or(0);
        let repairable = findings
            .get("findings")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter(|finding| finding.get("repair").is_some())
                    .count()
            })
            .unwrap_or(0);
        rows.push(InstallRow {
            id: install.install_id,
            name: o824_present::form_name(&install.kind, locale),
            version: install.version,
            findings: count,
            repairable,
        });
    }

    let described: Vec<Installation> = rows
        .iter()
        .map(|row| Installation {
            kind: row.id.clone(),
            version: row.version.clone(),
            findings: row.findings,
            repairable: row.repairable,
        })
        .collect();
    let copy = first_use(&described, locale).copy().clone();
    Ok(Screen {
        headline: copy.headline,
        reason: copy.reason,
        next_step: copy.next_step,
        installations: rows,
    })
}

/// The findings for one installation, each as a sentence with a next step.
#[tauri::command]
async fn diagnose(
    kernel: tauri::State<'_, Kernel>,
    install_id: String,
) -> Result<Vec<FoundRow>, String> {
    let locale = Locale::from_system(std::env::var("LANG").ok().as_deref());
    let mut guard = kernel.0.lock().map_err(|_| "the kernel handle is poisoned")?;
    let Some(bridge) = guard.as_mut() else {
        return Err("the kernel is not running".to_owned());
    };
    let envelope = bridge
        .doctor(&install_id)
        .map_err(|error| error.to_string())?;
    let findings = envelope
        .get("findings")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(findings
        .iter()
        .map(|finding| {
            let check = finding
                .get("checkId")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let repairable = finding.get("repair").is_some();
            let copy = o824_present::describe(check, repairable, locale);
            FoundRow {
                check: check.to_owned(),
                headline: copy.headline,
                reason: copy.reason,
                next_step: copy.next_step,
                repairable,
            }
        })
        .collect())
}

/// One finding, as the window shows it.
#[derive(Serialize)]
struct FoundRow {
    check: String,
    headline: String,
    reason: String,
    next_step: String,
    repairable: bool,
}

/// Where the kernel binary is.
///
/// `O824_KERNEL` wins, then a copy beside the window, then the workspace build the window
/// was built from, then `PATH`. A window that cannot find its kernel says so and points at
/// the command line rather than guessing.
fn resolve_kernel() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Some(explicit) = std::env::var_os("O824_KERNEL") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let beside = exe.with_file_name(format!("o824{}", std::env::consts::EXE_SUFFIX));
    if beside.is_file() {
        return Some(beside);
    }
    // Development layout: apps/o824-gui/target/debug/o824-gui.exe → ../../target/<profile>/o824.exe
    let mut directory = exe.parent()?.to_path_buf();
    for _ in 0..4 {
        for profile in ["release", "debug"] {
            for layout in [vec![profile], vec!["target", profile]] {
                let mut candidate = directory.clone();
                for part in &layout {
                    candidate = candidate.join(part);
                }
                let candidate = candidate.join(format!("o824{}", std::env::consts::EXE_SUFFIX));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        directory = directory.parent()?.to_path_buf();
    }
    Some(PathBuf::from("o824"))
}

/// Says, in a native box, that the window could not start and where the command line is.
#[allow(unsafe_code)]
fn report_start_failure(detail: &str) {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

        let text = HSTRING::from(format!(
            "The window could not start.\n\n{detail}\n\nThe command line still works: o824 doctor"
        ));
        let caption = HSTRING::from("O824");
        // SAFETY: a message box with two owned strings and no other pointers.
        unsafe {
            let _ = MessageBoxW(None, &text, &caption, MB_OK | MB_ICONERROR);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("the window could not start: {detail}");
    }
}
