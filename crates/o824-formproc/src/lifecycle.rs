//! Starting and stopping a form, with the guards that make it safe.
//!
//! Everything that decides *whether* to start or stop lives here, so the CLI and the
//! protocol cannot disagree: §21.5 makes start and stop plan actions, and a plan is only
//! worth having if running it does the same thing whoever built it.
//!
//! The three states a stop must tell apart:
//!
//! * the process is gone — the desired state is already reached, and the stale record
//!   should go;
//! * the pid now belongs to something else — the record is the only evidence this kernel
//!   started something here, so the stop is refused and the record is kept;
//! * the process is ours and alive — stop it, waiting until it is really gone.

use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_state::{ProcessRecord, State};

use crate::desktop;

/// The forms this build can start and stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `dsh --profile web`, a global npm package.
    Web,
    /// The installed desktop app.
    Desktop,
}

impl Form {
    /// The form an installation kind names, if this build starts it.
    pub fn of_kind(kind: &str) -> Option<Form> {
        match kind {
            "web" => Some(Form::Web),
            "desktop" => Some(Form::Desktop),
            _ => None,
        }
    }

    /// The state key a form's record is kept under.
    pub fn label(self) -> &'static str {
        match self {
            Form::Web => "web",
            Form::Desktop => "desktop",
        }
    }
}

/// What a start produced.
#[derive(Clone, Debug)]
pub struct Started {
    /// The process it spawned.
    pub pid: u32,
    /// The URL a web form printed; `None` for the desktop form.
    pub url: Option<String>,
    /// The port a web form listens on, when it named one.
    pub port: Option<u16>,
    /// The file its output goes to.
    pub log_path: PathBuf,
    /// The record written to the state file.
    pub record: ProcessRecord,
}

/// What a start produced.
///
/// `AlreadyRunning` is a **success**: the form someone asked for is running, and the pid is
/// the answer. It is not `E_CONCURRENT_INSTANCE` either — that code means "this home already
/// has an instance and you asked for a second one", which is a refusal. This is the case where
/// the desktop was started by someone else and the right answer is its pid.
#[derive(Clone, Debug)]
pub enum StartOutcome {
    /// We spawned it.
    Started(Started),
    /// Something was already running; nothing was spawned and nothing should be recorded.
    AlreadyRunning(u32),
}

/// What a stop did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    /// The process was alive, ours, and is now gone.
    Stopped(u32),
    /// The process was already gone; the record has been removed.
    AlreadyStopped(u32),
    /// The pid now belongs to a different process; nothing was killed.
    Refused(u32, String),
}

/// The live record, if any, that shares `home`.
///
/// §21.4's first risk is about the *home*, not about a label: two forms — or two starts
/// under different labels — sharing one DSH home share its sessions.
pub fn blocking_record<'a>(state: &'a State, home: &Path) -> Option<&'a ProcessRecord> {
    let home = home.display().to_string();
    state
        .processes
        .values()
        .find(|record| same_home(&record.home, &home) && is_alive(record))
}

/// True when the record still names a live process.
pub fn is_alive(record: &ProcessRecord) -> bool {
    crate::is_the_recorded_process(&record.image, record.created, record.pid)
}

/// Starts `form` for `home`, refusing when the home already has a live instance.
///
/// `record` is called as soon as the process exists, before readiness: an interrupted
/// start must leave something `stop` can find.
pub fn start(
    form: Form,
    executable: &Path,
    home: &Path,
    data_root: &Path,
    port: Option<u16>,
    record: &mut dyn FnMut(&ProcessRecord) -> Result<(), Error>,
) -> Result<StartOutcome, Error> {
    match form {
        Form::Web => {
            let started = crate::start(
                executable,
                home,
                data_root,
                port,
                crate::START_TIMEOUT,
                record,
            )?;
            Ok(StartOutcome::Started(Started {
                pid: started.pid,
                url: Some(started.url),
                port: started.port,
                log_path: started.log_path,
                record: started.record,
            }))
        }
        Form::Desktop => match desktop::start(executable, &[], home, data_root, record)? {
            desktop::StartOutcome::AlreadyRunning(pid) => Ok(StartOutcome::AlreadyRunning(pid)),
            desktop::StartOutcome::Started(started) => Ok(StartOutcome::Started(Started {
                pid: started.pid,
                url: None,
                port: None,
                log_path: started.log_path,
                record: started.record,
            })),
        },
    }
}

/// Stops the process recorded for `install_id`, applying the three-state rule.
pub fn stop(state: &mut State, install_id: &str) -> Result<StopOutcome, Error> {
    let Some(record) = state.processes.get(install_id).cloned() else {
        return Ok(StopOutcome::AlreadyStopped(0));
    };

    if crate::image_of(record.pid).is_none() {
        state.forget_process(install_id);
        return Ok(StopOutcome::AlreadyStopped(record.pid));
    }

    if !is_alive(&record) {
        return Ok(StopOutcome::Refused(
            record.pid,
            format!(
                "pid {} is not the process this kernel started; refusing to stop it",
                record.pid
            ),
        ));
    }

    crate::stop(&record)?;
    state.forget_process(install_id);
    Ok(StopOutcome::Stopped(record.pid))
}

/// True when two paths name the same directory, ignoring case and spelling.
pub fn same_home(left: &str, right: &str) -> bool {
    o824_model::path::clean(Path::new(left))
        .display()
        .to_string()
        .eq_ignore_ascii_case(
            &o824_model::path::clean(Path::new(right))
                .display()
                .to_string(),
        )
}

/// A refusal for a form this build does not start.
pub fn unsupported(kind: &str, reason: &str) -> Error {
    Error::new(
        Code::Unsupported,
        format!("the {kind} form cannot be started or stopped here: {reason}"),
    )
    .with_param("kind", kind.to_owned())
}
