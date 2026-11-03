//! The desktop form: starting the app the user asked for, or saying it is already there.
//!
//! §21.5's rule for this form is "start it if nothing is running, otherwise report what
//! is". Two instances of `desktop` cannot share a profile — DSH takes it exclusively — so
//! the answer to "start" when one is already up is the pid, not a second window.
//!
//! Readiness here is weaker than the web form's, and it is stated rather than faked: the
//! app prints no URL, so "started" means the process it spawned is still alive a moment
//! later. A window on screen is not something a kernel can see.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use o824_errs::{Code, Error};
use o824_state::ProcessRecord;

/// How long the app must stay alive before the start is reported as one.
pub const SETTLE: Duration = Duration::from_millis(1500);

/// What a successful start produced.
#[derive(Clone, Debug)]
pub struct Started {
    /// The process it spawned.
    pub pid: u32,
    /// The record to persist.
    pub record: ProcessRecord,
    /// The file its output goes to.
    pub log_path: PathBuf,
}

/// Starts the desktop app at `exe` with `home`, detached, and records what it started.
///
/// The caller writes the record from `on_spawned` as soon as the pid exists: an
/// interrupted start must leave something `stop` can find.
pub fn start(
    exe: &Path,
    args: &[String],
    home: &Path,
    data_root: &Path,
    on_spawned: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
) -> Result<Started, Error> {
    if !exe.is_file() {
        return Err(Error::new(
            Code::InstallDir,
            format!(
                "desktop: {:?} is not an executable",
                exe.display().to_string()
            ),
        ));
    }

    let log_path = crate::log_file(data_root, "desktop")?;
    let log = std::fs::File::create(&log_path)
        .map_err(|error| crate::file_error("create the log file", &log_path, &error))?;
    let log_for_stderr = log
        .try_clone()
        .map_err(|error| crate::file_error("share the log file", &log_path, &error))?;

    // The app runs from its own directory and is told which home to use, so the desktop it
    // boots is the one the kernel is reasoning about.
    let Some(install_dir) = exe.parent() else {
        return Err(Error::new(
            Code::InstallDir,
            format!("desktop: {:?} has no directory", exe.display().to_string()),
        ));
    };
    let mut child = o824_execx::spawn_detached(
        &exe.display().to_string(),
        args,
        Some(install_dir),
        Some(&[("DSH_HOME".to_owned(), home.display().to_string())]),
        log,
        log_for_stderr,
    )?;
    let pid = child.id();

    let Some(created) = crate::started_at(pid) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Error::new(
            Code::Internal,
            format!("desktop: the started process (pid {pid}) has no readable creation time; it was stopped"),
        ));
    };
    let record = ProcessRecord {
        install_id: String::new(),
        pid,
        port: None,
        home: home.display().to_string(),
        image: crate::image_of(pid).unwrap_or_default(),
        created,
        started_at: o824_model::Timestamp::now().to_rfc3339_seconds(),
        log_path: log_path.display().to_string(),
    };
    if let Err(error) = on_spawned(&record) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    // "Still alive" is the only readiness this form can report, and it is worth reporting:
    // an app that dies on startup must not be recorded as running.
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Error::new(
                Code::Internal,
                format!(
                    "desktop: the app exited ({status}) right after starting; see {}",
                    log_path.display()
                ),
            )
            .with_param("log", log_path.display().to_string()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(Started {
        pid,
        record,
        log_path,
    })
}
