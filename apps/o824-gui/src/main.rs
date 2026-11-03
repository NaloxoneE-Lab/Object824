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

fn main() {
    // Before Tauri starts: the data directory it will use has to be ours, not one an
    // elevated run left behind.
    prepare_webview_data();

    let result = tauri::Builder::default()
        .manage(Kernel(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![screen, diagnose])
        .run(tauri::generate_context!());

    if let Err(error) = result {
        // §7.4: a window that cannot start must say where the command line is, because the
        // command line still works.
        report_start_failure(&error.to_string());
    }
}

/// Points WebView2 at a directory this user owns, creating it if needed.
///
/// WebView2 defaults to `%LOCALAPPDATA%\<identifier>`. If that folder was created by an
/// elevated run it carries a high integrity level, and a normal run gets access denied
/// before the first pixel. Choosing the path here and creating it as this user removes the
/// whole class of failure; when even that fails, the app reports it instead of crashing.
fn prepare_webview_data() {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let directory = std::path::PathBuf::from(local)
        .join("O824")
        .join("WebView2");
    if std::fs::create_dir_all(&directory).is_ok() {
        // WebView2 reads this before the window is created.
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &directory);
    }
}

/// The first screen: what is installed, and what the main button should say.
#[tauri::command]
fn screen(kernel: tauri::State<Kernel>) -> Result<Screen, String> {
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
fn diagnose(kernel: tauri::State<Kernel>, install_id: String) -> Result<Vec<FoundRow>, String> {
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
            let candidate = directory.join(profile).join(format!("o824{}", std::env::consts::EXE_SUFFIX));
            if candidate.is_file() {
                return Some(candidate);
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
