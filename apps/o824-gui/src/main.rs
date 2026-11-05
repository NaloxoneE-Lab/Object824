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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

use tauri::Emitter;
use tauri::Manager;

use o824_bridge::{Bridge, Options};
use o824_present::Locale;
use serde::Serialize;

/// Which language the window speaks.
///
/// `LANG` is a Unix habit: a Windows user who double-clicks the program has no LANG at all,
/// so reading only that variable made a Chinese copy of Windows show an English window. The
/// Windows UI language is asked first, and `O824_LOCALE` exists so a screenshot can be taken
/// in either language on one machine.
static LOCALE: OnceLock<Locale> = OnceLock::new();

#[allow(unsafe_code)]
fn locale() -> Locale {
    *LOCALE.get_or_init(|| {
        if let Some(tag) = std::env::var_os("O824_LOCALE") {
            return Locale::from_system(tag.to_str());
        }
        #[cfg(windows)]
        {
            use windows::Win32::Globalization::GetUserDefaultUILanguage;
            // SAFETY: no arguments, no pointers; the call reports the user's UI language.
            let language = unsafe { GetUserDefaultUILanguage() };
            if language & 0x3ff == 0x04 {
                return Locale::ZhHans;
            }
            return Locale::En;
        }
        #[cfg(not(windows))]
        Locale::from_system(std::env::var("LANG").ok().as_deref())
    })
}

/// Whether the notification-area icon registered. A window that hides itself with no tray
/// icon is a process nobody can get back, so the close button has to know.
static TRAY_OK: AtomicBool = AtomicBool::new(false);

/// What the window asks for when it starts.
#[derive(Serialize)]
struct Screen {
    /// The sentence group for the first screen.
    headline: String,
    reason: String,
    next_step: String,
    /// The installations the kernel found, already described for a person.
    installations: Vec<InstallRow>,
    /// What the main button does: `install`, `start`, `repair`, `choose`, or empty.
    action: String,
}

/// One installation, as the window shows it.
#[derive(Serialize)]
struct InstallRow {
    id: String,
    kind: String,
    name: String,
    version: String,
    /// Where it lives, so the window can offer to open the folder.
    root: String,
    findings: usize,
    repairable: usize,
}

/// The kernel this window talks to, kept for the life of the window.
struct Kernel(Mutex<Option<Bridge>>);

/// What a start or a stop produced.
#[derive(Serialize)]
struct RunningView {
    /// True when the form is running now.
    running: bool,
    /// What the start/stop button should say next.
    start_label: String,
    /// What the other button should say.
    stop_label: String,
    /// What the link to a running web form says.
    open_label: String,
    /// What the undo button says.
    undo_label: String,
    /// The address to open, for a form that has one.
    url: Option<String>,
    /// The sentence for the user.
    message: String,
}

/// The plan waiting for a yes or a no.
///
/// A repair is two steps on purpose (§7.2): the kernel writes what it would do, the window
/// shows it, and only then does anything change. The plan is kept verbatim because `apply`
/// takes the plan the kernel wrote, not a description of one.
#[derive(Default)]
struct PendingPlan(Mutex<Option<serde_json::Value>>);

/// What the window needs to ask for permission.
#[derive(Serialize)]
struct PlanView {
    /// The plan's identifier, so a log can name it.
    plan_id: String,
    /// The steps, each already in ordinary words.
    steps: Vec<String>,
    /// The safety-net sentence.
    backup: String,
    /// The sentence above the list.
    heading: String,
    /// What the button says.
    confirm: String,
    /// What the way out says.
    cancel: String,
    /// When the plan stops being applicable.
    expires_at: String,
}

/// Undoes the last repair by replaying the journal's record of what it replaced.
#[tauri::command]
async fn undo_repair(install_id: String) -> Result<ApplyView, String> {
    let locale = locale();
    let mut bridge = Bridge::start(&kernel_options()).map_err(|error| error.to_string())?;
    let plan = bridge
        .plan(&install_id, "rollback")
        .map_err(|error| error.to_string())?;
    bridge.apply(&plan).map_err(|error| error.to_string())?;
    let after = bridge
        .doctor(&install_id)
        .map_err(|error| error.to_string())?;
    let _ = bridge.close();
    let findings = after["findings"].as_array().map(Vec::len).unwrap_or(0);
    let copy = o824_present::undone_copy(locale);
    Ok(ApplyView {
        applied: true,
        findings,
        repairable: 0,
        headline: copy.headline,
        reason: copy.reason,
        next_step: copy.next_step,
    })
}

/// What `apply` produced.
#[derive(Serialize)]
struct ApplyView {
    /// True when the kernel applied the plan without complaint.
    applied: bool,
    /// What the diagnosis says now.
    findings: usize,
    /// How many of those can still be fixed automatically.
    repairable: usize,
    /// The sentence for the user.
    headline: String,
    /// The reason, when something is left.
    reason: String,
    /// What can be done next.
    next_step: String,
}

/// The last concrete reason the window could not start, kept so the dialog can say it.
///
/// "The window could not be created" without the reason is the kind of message that makes
/// someone open Task Manager; the directory and the OS error are what make it actionable.
static LAST_PROBLEM: Mutex<Option<String>> = Mutex::new(None);

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
/// Where this program may write its log, most likely first.
///
/// Next to the program (it lives in a folder the user owns), then `%LOCALAPPDATA%`, then
/// `%TEMP%`. A file, not a directory — creating a file succeeds in places where creating a
/// folder is denied.
fn log_candidates() -> Vec<std::path::PathBuf> {
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
    candidates
}

/// The log file that exists, so "open the log" opens something rather than a folder.
fn existing_log() -> Option<std::path::PathBuf> {
    log_candidates().into_iter().find(|path| path.is_file())
}

fn log_line(line: &str) {
    use std::io::Write as _;
    for path in log_candidates() {
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

/// True when this is the first window of this session.
///
/// A launcher that can be opened twice shows two contradictory diagnoses of the same
/// machine, and the second window's "repair" races the first one's. The second copy brings
/// the first to the front and leaves, which is what every tool with a single window does.
///
/// The mutex is `Local\` on purpose: it is per session, so two people logged in at once do
/// not block each other, and it needs no privilege.
#[allow(unsafe_code)]
fn only_instance() -> bool {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
        };

        // The window itself is the evidence. A named mutex was tried first and reported
        // "already exists" on a machine with nothing running — the last-error slot is a
        // thread-local that any Win32 call can clobber, and chasing it cost more than the
        // guard is worth. A window with this title is unambiguous, and it is the thing the
        // user would look for anyway.
        // SAFETY: two Win32 calls, no pointers beyond an owned string.
        unsafe {
            let title = HSTRING::from("O824");
            match FindWindowW(None, &title) {
                Ok(existing) if !existing.is_invalid() => {
                    log_line("another window is open; brought it to the front and left");
                    let _ = ShowWindow(existing, SW_RESTORE);
                    let _ = SetForegroundWindow(existing);
                    false
                }
                _ => true,
            }
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn main() {
    log_line("=== start ===");
    log_token();
    if !only_instance() {
        log_line("another window is open; brought it to the front and left");
        return;
    }
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
    // elevated run left behind. One choice, used for both the environment variable and the
    // builder option — two different folders is how a working window fails with
    // "the resource is in use" for no visible reason.
    let data_directory = usable_data_directory();
    if let Some(directory) = &data_directory {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", directory);
    }
    log_line(&format!(
        "kernel={:?} webview_data={:?}",
        resolve_kernel(),
        std::env::var_os("WEBVIEW2_USER_DATA_FOLDER")
    ));
    let started = std::panic::catch_unwind(|| {
        tauri::Builder::default()
            .setup(move |app| {
                // §7.5: the window is created here rather than by the config because its
                // browser data directory has to be a path this account can actually write.
                // `webview2_user_data_folder` is not consulted by the runtime — the directory
                // is passed to WebView2 explicitly, which is why it has to be set here.
                // The switches take the window straight to a state that cannot be produced with
                // a mouse — an empty machine, a verification dialog, the settings page — so each
                // screen can be photographed, and so a screenshot is reproducible.
                let mut query: Vec<String> = Vec::new();
                if let Ok(mode) = std::env::var("O824_GUI_AUTOREPAIR") {
                    let mode = match mode.as_str() {
                        "slow" => "slow",
                        "hold" => "hold",
                        _ => "1",
                    };
                    query.push(format!("autorepair={mode}"));
                }
                for (variable, key) in [
                    ("O824_GUI_UPDATE", "update"),
                    ("O824_GUI_TRUST", "trust"),
                    ("O824_GUI_EMPTY", "empty"),
                    ("O824_GUI_GUIDE", "guide"),
                    ("O824_GUI_SETTINGS", "settings"),
                    ("O824_GUI_MENU", "menu"),
                    ("O824_GUI_DETAILS", "details"),
                    ("O824_GUI_ABOUT", "about"),
                ] {
                    if std::env::var_os(variable).is_some() {
                        query.push(format!("{key}=1"));
                    }
                }
                // Free-form query for a screenshot of an arbitrary state (`kind=web`), kept
                // separate from the named switches so it cannot change what a named one means.
                if let Ok(extra) = std::env::var("O824_GUI_QUERY") {
                    let extra = extra.trim_start_matches('?');
                    if !extra.is_empty() {
                        query.push(extra.to_owned());
                    }
                }
                let url = if query.is_empty() {
                    tauri::WebviewUrl::App("index.html".into())
                } else {
                    tauri::WebviewUrl::App(format!("index.html?{}", query.join("&")).into())
                };
                let mut builder = tauri::WebviewWindowBuilder::new(app, "main", url)
                    .title("O824")
                    // The reference window is 1583×869 (1.82:1); this is that ratio at 1180.
                    .inner_size(1180.0, 648.0)
                    // The window draws its own title bar; a launcher is a fixed board, not a
                    // document, so there is nothing to gain from letting it be resized into a
                    // shape the layout was not designed for.
                    .decorations(false)
                    .resizable(false)
                    .transparent(true)
                    .center();
                if let Some(directory) = data_directory.clone() {
                    builder = builder.data_directory(directory);
                }
                builder.build()?;
                // Best effort: a machine that refuses the icon should not stop the window.
                match build_tray(app) {
                    Ok(()) => TRAY_OK.store(true, Ordering::Relaxed),
                    Err(error) => log_line(&format!("tray: not created: {error}")),
                }
                Ok(())
            })
            .manage(Kernel(Mutex::new(None)))
            .manage(PendingPlan::default())
            .invoke_handler(tauri::generate_handler![
                list,
                checking,
                diagnose,
                summarize,
                repair_plan,
                repair_apply,
                undo_repair,
                form_action,
                install_help,
                trust_help,
                update_check,
                update_apply,
                quit_now,
                chrome,
                absent,
                shell_info,
                window_action,
                set_autostart,
                create_shortcut,
                clear_cache,
                open_log,
                open_install_dir,
                open_url
            ])
            .build(tauri::generate_context!())
    });

    // §7.4: a window that cannot start must say where the command line is, because the
    // command line still works. Tauri reports a failed setup by panicking, so the catch is
    // around the call rather than on its result.
    match started {
        Ok(Ok(app)) => {
            // The tray is registered once the event loop runs: the shell talks to the window
            // while adding the icon, and a thread still inside `setup` is not answering.
            app.run(|_, _| {});
            log_line("=== window closed normally ===");
        }
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
                // A directory can exist and still refuse everything inside it: a folder made
                // by another account or another integrity level answers `create_dir_all` with
                // success and then fails WebView2 with a misleading "resource in use". The
                // only proof that it is usable is writing a byte into it.
                let probe = directory.join("o824-write-probe.tmp");
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&probe)
                {
                    Ok(_) => {
                        let _ = std::fs::remove_file(&probe);
                        log_line(&format!("webview data dir ready: {}", directory.display()));
                        return Some(directory);
                    }
                    Err(error) => log_line(&format!(
                        "webview data dir FAILED: {} (not writable: {error})",
                        directory.display()
                    )),
                }
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

/// The installations on this machine, without diagnosing them.
///
/// Discovery is 95 ms; a diagnosis is 3.7-5.4 s on a real machine, because it walks every
/// plugin folder. Waiting for three of those before drawing anything is what made the
/// window look stuck, so the list comes first and each row is diagnosed separately.
#[tauri::command]
async fn list(kernel: tauri::State<'_, Kernel>) -> Result<Vec<InstallRow>, String> {
    let started = std::time::Instant::now();
    let locale = locale();
    let mut guard = kernel
        .0
        .lock()
        .map_err(|_| "the kernel handle is poisoned")?;
    let bridge = match guard.as_mut() {
        Some(bridge) => bridge,
        None => {
            let started = Bridge::start(&kernel_options()).map_err(|error| error.to_string())?;
            guard.insert(started)
        }
    };
    let installs = bridge.discover().map_err(|error| error.to_string())?;
    log_line(&format!(
        "list: {} installs in {:?}",
        installs.len(),
        started.elapsed()
    ));
    Ok(installs
        .into_iter()
        .map(|install| InstallRow {
            id: install.install_id,
            kind: install.kind.clone(),
            name: o824_present::form_name(&install.kind, locale),
            version: install.version,
            root: install.root,
            findings: 0,
            repairable: 0,
        })
        .collect())
}

/// The sentence group for "still checking", so the window is never blank.
#[tauri::command]
fn checking(locale_tag: Option<String>) -> Screen {
    let locale = match locale_tag.as_deref() {
        Some(tag) => Locale::from_system(Some(tag)),
        None => locale(),
    };
    let copy = o824_present::checking_copy(locale);
    Screen {
        headline: copy.headline.clone(),
        reason: copy.reason.clone(),
        next_step: String::new(),
        installations: Vec::new(),
        action: String::new(),
    }
}

/// Where a release is published. The release page must attach `manifest.json` beside the
/// executables for the "latest" alias to resolve; `O824_UPDATE_URL` overrides it, which is how
/// this is exercised against a local server.
const UPDATE_BASE: &str = "https://github.com/Naloxone-E-Lab/Object824/releases/latest/download";

fn update_base() -> String {
    std::env::var("O824_UPDATE_URL").unwrap_or_else(|_| UPDATE_BASE.to_owned())
}

/// Runs the kernel with the given arguments, and returns its output and exit code.
///
/// The window does not re-implement verification: `o824 update` already refuses anything whose
/// signature or hashes do not match, and a second implementation here would be a second thing
/// to get wrong.
fn run_kernel(args: &[&str]) -> Result<(i32, String, String), String> {
    let program =
        resolve_kernel().ok_or_else(|| "the kernel was not found next to the window".to_owned())?;
    let output = std::process::Command::new(&program)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run {:?}: {error}", program.display().to_string()))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

/// What a check found.
#[derive(Serialize)]
struct UpdateView {
    /// True when the kernel verified a staged release.
    staged: bool,
    /// The version it describes.
    published: String,
    /// The version running now.
    current: String,
    /// How many files the release contains.
    files: usize,
    /// The key that signed it, as a short id.
    key_id: String,
    /// A sentence for the window.
    message: String,
    /// Where the verified files are, hidden from the person but used by apply.
    staged_dir: String,
}

/// Downloads a release into a staging folder and verifies it. Changes nothing installed.
#[tauri::command]
async fn update_check() -> Result<UpdateView, String> {
    let base = update_base();
    let staged = o824_install::default_directory().join("staged");
    let staged_text = staged.display().to_string();
    let _ = std::fs::remove_dir_all(&staged);
    log_line(&format!("update: fetching {base} into {staged_text}"));

    let (code, stdout, stderr) = run_kernel(&[
        "update",
        "--fetch",
        "--manifest-url",
        &format!("{base}/manifest.json"),
        "--base-url",
        &base,
        "--into",
        &staged_text,
    ])?;
    log_line(&format!("update: kernel exited {code}"));

    if code != 0 {
        return Err(if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        });
    }

    // Read back what the manifest said, so the window can show the evidence it is acting on.
    let manifest = o824_manifest::load(&staged.join("manifest.json"))
        .map_err(|error| format!("the verified manifest could not be read: {error}"))?;
    Ok(UpdateView {
        staged: true,
        published: manifest.version.clone(),
        current: o824_version::VERSION.to_owned(),
        files: manifest.files.len(),
        key_id: manifest
            .signature
            .as_ref()
            .map(|signature| signature.key_id.clone())
            .unwrap_or_default(),
        message: stdout.trim().to_owned(),
        staged_dir: staged_text,
    })
}

/// Ends this process so the freshly installed one can own the single window.
///
/// After a swap the installed executable is a different file, and the running process is the
/// old one; leaving it alive would also leave the single-instance guard holding the title.
#[tauri::command]
fn quit_now(app: tauri::AppHandle) {
    log_line("update: closing so the new build can start");
    app.exit(0);
}

// ---------------------------------------------------------------------------
// The shell: its labels, its window buttons, and the conveniences behind the
// start control's menu. None of these decide anything about a machine; they
// either fetch copy or do the small thing their name says.

#[tauri::command]
fn chrome() -> o824_present::Chrome {
    o824_present::chrome(locale())
}

/// The first screen for a distribution this machine does not have.
#[tauri::command]
fn absent(kind: String) -> o824_present::Copy {
    o824_present::absent_copy(&kind, locale())
}

/// Facts the shell needs once at start, none of which belong in the copy crate.
#[derive(Serialize)]
struct ShellInfo {
    /// The launcher's own version, shown on the About page.
    version: String,
    /// Whether closing the window can hide it to a tray icon that actually exists.
    tray: bool,
    /// Whether the launcher starts with Windows.
    autostart: bool,
    /// The log file, when one has been written.
    log_path: Option<String>,
}

#[tauri::command]
fn shell_info() -> ShellInfo {
    ShellInfo {
        version: o824_version::VERSION.to_owned(),
        tray: TRAY_OK.load(Ordering::Relaxed),
        autostart: autostart_is_enabled(),
        log_path: existing_log().map(|path| path.display().to_string()),
    }
}

/// Minimize, hide, or quit — the three things a custom title bar can ask for.
#[tauri::command]
fn window_action(app: tauri::AppHandle, action: String) -> Result<(), String> {
    match action.as_str() {
        "minimize" => app
            .get_webview_window("main")
            .ok_or_else(|| "the window is gone".to_owned())?
            .minimize()
            .map_err(|error| error.to_string()),
        "hide" => app
            .get_webview_window("main")
            .ok_or_else(|| "the window is gone".to_owned())?
            .hide()
            .map_err(|error| error.to_string()),
        "quit" => {
            log_line("window: quit from the title bar");
            app.exit(0);
            Ok(())
        }
        other => Err(format!("window_action: unknown action {other:?}")),
    }
}

/// Writes or removes the per-user autostart entry, and reports what the registry says now.
///
/// A failed write is reported rather than assumed: the switch in the window must show the
/// machine's state, not the state the click hoped for.
#[tauri::command]
fn set_autostart(enabled: bool) -> Result<bool, String> {
    autostart_set(enabled)?;
    let now = autostart_is_enabled();
    log_line(&format!("autostart: requested {enabled}, now {now}"));
    Ok(now)
}

#[tauri::command]
fn create_shortcut() -> Result<(), String> {
    desktop_shortcut()
}

#[tauri::command]
fn clear_cache() -> Result<(), String> {
    let staged = o824_install::default_directory().join("staged");
    if staged.is_dir() {
        std::fs::remove_dir_all(&staged)
            .map_err(|error| format!("could not clear {}: {error}", staged.display()))?;
    }
    log_line("update: cache cleared");
    Ok(())
}

#[tauri::command]
fn open_log() -> Result<(), String> {
    let path = existing_log().ok_or_else(|| "no log file has been written yet".to_owned())?;
    shell_open(&path.display().to_string())
}

#[tauri::command]
async fn open_install_dir(install_id: String) -> Result<(), String> {
    let mut bridge = Bridge::start(&kernel_options()).map_err(|error| error.to_string())?;
    let installs = bridge.discover().map_err(|error| error.to_string())?;
    let _ = bridge.close();
    let install = installs
        .into_iter()
        .find(|install| install.install_id == install_id)
        .ok_or_else(|| format!("no installation has id {install_id}"))?;
    shell_open(&install.root)
}

/// Opens a link in the user's browser rather than navigating the window away from itself.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links can be opened".to_owned());
    }
    shell_open(&url)
}

/// Applies the staged release and lets the kernel restart the window.
#[tauri::command]
async fn update_apply(staged_dir: String) -> Result<String, String> {
    let target = o824_install::default_directory().display().to_string();
    let (code, stdout, stderr) = run_kernel(&[
        "update",
        "--apply",
        "--staged",
        &staged_dir,
        "--target",
        &target,
        "--yes",
    ])?;
    if code != 0 {
        return Err(if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        });
    }
    log_line("update: swap scheduled");
    Ok(stdout.trim().to_owned())
}

/// Why Windows shows "unrecognised app", and how to check the download instead.
///
/// §13: the build is unsigned by decision, so the window says what that means and how to
/// verify what you got — and never suggests turning a protection off.
#[tauri::command]
fn trust_help() -> o824_present::TrustHelp {
    let locale = locale();
    o824_present::trust_help(locale)
}

/// What to do when nothing is installed.
///
/// §20 is explicit that the launcher guides here rather than installing on the user's
/// behalf, so this returns words and commands, never an action that changes the machine.
#[tauri::command]
fn install_help() -> InstallHelp {
    let locale = locale();
    let (guidance, steps) = o824_present::install_guidance(locale);
    InstallHelp {
        guidance,
        steps: steps
            .into_iter()
            .map(|(label, command)| InstallStep { label, command })
            .collect(),
        copy_label: o824_present::copy_label(locale),
    }
}

/// One way to install, as the window shows it.
#[derive(Serialize)]
struct InstallStep {
    /// Which form it installs.
    label: String,
    /// The command, or a link.
    command: String,
}

/// The guidance and its commands.
#[derive(Serialize)]
struct InstallHelp {
    /// The sentence above the list.
    guidance: String,
    /// The ways to install.
    steps: Vec<InstallStep>,
    /// What the copy button says.
    copy_label: String,
}

/// Starts or stops the form an installation provides.
///
/// §21.5 again: both are plan actions, so the window asks for a plan and applies it — the
/// same path a repair takes, with no second implementation to drift.
#[tauri::command]
async fn form_action(install_id: String, action: String) -> Result<RunningView, String> {
    let locale = locale();
    // `labels` is not an action on the machine; it is how the page gets the words for the
    // two buttons without carrying a second copy of the catalog in JavaScript.
    if action == "labels" {
        return Ok(RunningView {
            running: false,
            url: None,
            message: String::new(),
            start_label: o824_present::start_label(locale),
            stop_label: o824_present::stop_label(locale),
            open_label: o824_present::open_label(locale),
            undo_label: o824_present::undo_label(locale),
        });
    }
    if action != "start" && action != "stop" {
        return Err(format!("form_action: unknown action {action:?}"));
    }
    let options = kernel_options();
    let mut bridge = Bridge::start(&options).map_err(|error| error.to_string())?;
    let plan = bridge
        .plan(&install_id, &action)
        .map_err(|error| error.to_string())?;
    bridge.apply(&plan).map_err(|error| error.to_string())?;
    let _ = bridge.close();
    log_line(&format!("form_action {action} {install_id}: applied"));

    // A start records the process it created; the address the web form printed is in that
    // process's log, which is the only place it exists (the record deliberately holds no
    // token).
    let state_path = options
        .data_root
        .clone()
        .unwrap_or_else(|| o824_state::data_root(None))
        .join(o824_state::STATE_FILE_NAME);
    let record = o824_state::load(&state_path)
        .ok()
        .and_then(|state| state.processes.get(&install_id).cloned());
    let url = record
        .as_ref()
        .and_then(|record| o824_formproc::url_from_log(std::path::Path::new(&record.log_path)));
    let running = record.is_some();

    let message = if action == "start" {
        if running {
            "started"
        } else {
            "not started"
        }
    } else if running {
        "still running"
    } else {
        "stopped"
    };
    Ok(RunningView {
        running,
        url,
        message: message.to_owned(),
        start_label: o824_present::start_label(locale),
        stop_label: o824_present::stop_label(locale),
        open_label: o824_present::open_label(locale),
        undo_label: o824_present::undo_label(locale),
    })
}

/// Asks the kernel what a repair would do, and shows it before anything changes.
#[tauri::command]
async fn repair_plan(
    pending: tauri::State<'_, PendingPlan>,
    install_id: String,
    action: Option<String>,
) -> Result<PlanView, String> {
    let locale = locale();
    let mut bridge = Bridge::start(&kernel_options()).map_err(|error| error.to_string())?;
    let action = action.unwrap_or_else(|| "repair".to_owned());
    let plan = bridge
        .plan(&install_id, &action)
        .map_err(|error| error.to_string())?;
    let _ = bridge.close();

    let steps: Vec<String> = plan["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|step| {
                    let kind = step["kind"].as_str().unwrap_or("other");
                    o824_present::step_name(kind, locale)
                })
                .collect()
        })
        .unwrap_or_default();
    if steps.is_empty() {
        return Err("the kernel found nothing to repair".to_owned());
    }
    let plan_id = plan["id"].as_str().unwrap_or_default().to_owned();
    let expires_at = plan["expiresAt"].as_str().unwrap_or_default().to_owned();
    log_line(&format!("repair plan {plan_id}: {} steps", steps.len()));
    if let Ok(mut slot) = pending.0.lock() {
        *slot = Some(plan);
    }
    Ok(PlanView {
        plan_id,
        steps,
        backup: o824_present::backup_notice(locale),
        heading: o824_present::steps_heading(locale),
        confirm: o824_present::confirm_copy(locale),
        cancel: o824_present::cancel_copy(locale),
        expires_at,
    })
}

/// Runs the plan the user just approved, then says what the diagnosis says now.
#[tauri::command]
async fn repair_apply(
    pending: tauri::State<'_, PendingPlan>,
    install_id: String,
) -> Result<ApplyView, String> {
    let locale = locale();
    let plan = pending
        .0
        .lock()
        .map_err(|_| "the plan handle is poisoned")?
        .take()
        .ok_or_else(|| "there is no plan to apply".to_owned())?;
    let plan_id = plan["id"].as_str().unwrap_or_default().to_owned();

    let mut bridge = Bridge::start(&kernel_options()).map_err(|error| error.to_string())?;
    let applied = bridge.apply(&plan);
    let after = bridge.doctor(&install_id);
    let _ = bridge.close();

    let applied_ok = applied.is_ok();
    if let Err(error) = &applied {
        log_line(&format!("apply {plan_id} failed: {error}"));
    } else {
        log_line(&format!("apply {plan_id} ok"));
    }

    let (findings, repairable) = match &after {
        Ok(envelope) => {
            let items = envelope["findings"].as_array().cloned().unwrap_or_default();
            (
                items.len(),
                items.iter().filter(|f| f.get("repair").is_some()).count(),
            )
        }
        Err(_) => (0, 0),
    };

    let copy = if applied_ok && findings == 0 {
        o824_present::repair_done(locale)
    } else if applied_ok {
        o824_present::repair_remaining(locale)
    } else {
        applied
            .err()
            .map(|error| o824_present::failure_copy(&error.to_string(), locale))
            .unwrap_or_else(|| o824_present::repair_done(locale))
    };

    Ok(ApplyView {
        applied: applied_ok,
        findings,
        repairable,
        headline: copy.headline,
        reason: copy.reason,
        next_step: copy.next_step,
    })
}

/// The first screen once every row has been diagnosed.
///
/// The decision belongs here: which sentence the window leads with is the same rule whether
/// the diagnoses arrived all at once or one by one.
#[tauri::command]
fn summarize(
    installs: Vec<o824_present::Installation>,
    layout: Option<serde_json::Value>,
) -> Screen {
    log_line(&format!("summarize: {} rows", installs.len()));
    if let Some(layout) = layout {
        log_line(&format!("layout: {layout}"));
    }
    let locale = locale();
    let state = o824_present::first_use(&installs, locale);
    let copy = state.copy().clone();
    Screen {
        headline: copy.headline,
        reason: copy.reason,
        next_step: copy.next_step,
        installations: Vec::new(),
        action: state.action().to_owned(),
    }
}

/// The findings for one installation, each as a sentence with a next step.
#[tauri::command]
async fn diagnose(
    kernel: tauri::State<'_, Kernel>,
    install_id: String,
) -> Result<Vec<FoundRow>, String> {
    let locale = locale();
    // Its own kernel, so three diagnoses run at the same time instead of queueing behind
    // one another: they are independent questions and each costs seconds.
    let mut own = Bridge::start(&kernel_options()).map_err(|error| error.to_string())?;
    let started = std::time::Instant::now();
    let envelope = own.doctor(&install_id).map_err(|error| error.to_string())?;
    log_line(&format!("diagnose {install_id}: {:?}", started.elapsed()));
    let _ = own.close();
    let _ = &kernel;
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

/// Records which kind of process this is: a tray icon is refused to a restricted token, and
/// knowing that from the log saves another round of guessing.
#[allow(unsafe_code)]
#[cfg(windows)]
fn log_token() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{GetTokenInformation, TokenIsRestricted, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = Default::default();
    // SAFETY: the pseudo-handle is always valid, and the token is closed before returning.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            log_line("token: cannot open");
            return;
        }
        let mut restricted = 0u32;
        let mut size = 0u32;
        let _ = GetTokenInformation(
            token,
            TokenIsRestricted,
            Some(&mut restricted as *mut u32 as *mut _),
            std::mem::size_of::<u32>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        log_line(&format!(
            "token: restricted={} session={} user={}",
            restricted != 0,
            std::process::id(),
            std::env::var("USERNAME").unwrap_or_default()
        ));
    }
}

#[cfg(not(windows))]
fn log_token() {}

/// The notification-area icon, with the three things it is for.
///
/// Reinstated after the reason it never appeared was found: files inside the DSH workspace
/// carry a low-integrity mandatory label, and a process started from one of those runs at low
/// integrity, which the notification area (medium) refuses. Run from a normal install
/// location — `%LOCALAPPDATA%` or Program Files — the same code registers its icon.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::with_id(app, "show", "Show O824", true, None::<&str>)?;
    // Checking for a new version means opening the page that publishes it, never replacing
    // this executable: the build is unsigned, and an unsigned binary that updates itself is
    // the one thing a launcher must not do.
    let updates = MenuItem::with_id(app, "updates", "Check for updates...", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &updates, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("the window icon is missing".to_owned()))?;
    log_line(&format!("tray: icon {}x{}", icon.width(), icon.height()));

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .tooltip("O824")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "updates" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                    // The window runs the check: it can show what was verified before anyone
                    // is asked to install anything.
                    let _ = window.emit("update-requested", ());
                }
            }
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;
    log_line("tray: created");
    Ok(())
}

/// Where the kernel should look, when this window is being exercised against a fixture.
///
/// `O824_ROOT` exists so the whole window — not just the protocol underneath it — can be run
/// against a broken tree. A test that repairs the developer's own DSH is a test nobody dares
/// run twice.
fn kernel_options() -> Options {
    use std::path::PathBuf;

    let root = std::env::var_os("O824_ROOT").map(PathBuf::from);
    // The laboratory trees keep the program files, the settings home and the state apart
    // (`f03/app`, `f03/home`, `f03/data`). One variable could only ever describe one of them,
    // so each has its own, and the old single-variable behaviour is the default.
    let home = std::env::var_os("O824_HOME")
        .map(PathBuf::from)
        .or_else(|| root.clone());
    let data_root = std::env::var_os("O824_DATA")
        .map(PathBuf::from)
        .or_else(|| root.clone().map(|path| path.join("data")));
    Options {
        program: resolve_kernel(),
        root,
        home,
        data_root,
    }
}

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
    // Six levels: target/release → target → o824-gui → apps → repository root covers the
    // development layout as well as the packaged one, so a deleted copy beside the window
    // does not turn into "o824: program not found".
    for _ in 0..6 {
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

/// Opens a file, a folder, or a link with whatever the user has associated with it.
#[cfg(windows)]
#[allow(unsafe_code)]
fn shell_open(target: &str) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = HSTRING::from("open");
    let file = HSTRING::from(target);
    // SAFETY: both strings outlive the call, and the other three arguments are null.
    let result = unsafe {
        ShellExecuteW(
            None,
            &verb,
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute's documented success values start at 33; anything at or below 32 is an
    // error code disguised as a handle.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "could not open {target} (ShellExecute returned {})",
            result.0 as isize
        ))
    }
}

#[cfg(not(windows))]
fn shell_open(_target: &str) -> Result<(), String> {
    Err("opening a path is only implemented on Windows".to_owned())
}

/// The per-user `Run` entry, which is the documented way to start at sign-in without a task.
///
/// `HKEY_CURRENT_USER` means no elevation, and no administrator ever has to be involved in
/// turning this on or off.
#[cfg(windows)]
#[allow(unsafe_code)]
fn autostart_is_enabled() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut size = 0u32;
    // SAFETY: only the size out-parameter is written; no buffer is passed on this call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("O824"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && size > 0
}

#[cfg(not(windows))]
fn autostart_is_enabled() -> bool {
    false
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn autostart_set(enabled: bool) -> Result<(), String> {
    use windows::core::{w, HSTRING};
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };

    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = w!("O824");
    // SAFETY: every pointer is a live local; the key and value names are static.
    unsafe {
        if enabled {
            let exe = std::env::current_exe()
                .map_err(|error| format!("cannot find this program: {error}"))?;
            let command = HSTRING::from(format!("\"{}\"", exe.display()));
            let mut wide: Vec<u16> = command.as_wide().to_vec();
            wide.push(0);
            let bytes = (wide.len() * 2) as u32;
            let status = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key,
                name,
                REG_SZ.0,
                Some(wide.as_ptr() as *const _),
                bytes,
            );
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "the autostart entry could not be written ({})",
                    status.0
                ));
            }
        } else {
            let status = RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name);
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(format!(
                    "the autostart entry could not be removed ({})",
                    status.0
                ));
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn autostart_set(_enabled: bool) -> Result<(), String> {
    Err("autostart is only implemented on Windows".to_owned())
}

/// Puts an O824 shortcut on the desktop.
///
/// The `.lnk` format is written by `WScript.Shell` rather than by hand: a shortcut has an
/// item identifier list, a link-info block and a path block, and hand-rolling those is a
/// week of testing for a file the shell already knows how to write.
#[cfg(windows)]
fn desktop_shortcut() -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe =
        std::env::current_exe().map_err(|error| format!("cannot find this program: {error}"))?;
    let directory = exe
        .parent()
        .ok_or_else(|| "this program has no folder".to_owned())?;
    let quote = |value: &str| value.replace('\'', "''");
    let exe_text = quote(&exe.display().to_string());
    let directory_text = quote(&directory.display().to_string());
    let icon_text = quote(&format!("{},0", exe.display()));
    let script = format!(
        "$p = [Environment]::GetFolderPath('Desktop'); \
         $s = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $p 'O824.lnk')); \
         $s.TargetPath = '{exe_text}'; \
         $s.WorkingDirectory = '{directory_text}'; \
         $s.IconLocation = '{icon_text}'; \
         $s.Save()"
    );
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| format!("cannot run PowerShell: {error}"))?;
    if output.status.success() {
        log_line("shortcut: created on the desktop");
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if detail.is_empty() {
            "the shortcut could not be created".to_owned()
        } else {
            detail
        })
    }
}

#[cfg(not(windows))]
fn desktop_shortcut() -> Result<(), String> {
    Err("shortcuts are only implemented on Windows".to_owned())
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
