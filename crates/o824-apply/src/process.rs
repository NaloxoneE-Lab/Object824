//! Running a plan's process steps: start a form, or stop the one this kernel started.
//!
//! The step carries everything the decision needs — the executable, the home, the port,
//! the installation — because a plan is written to be run later, possibly by another
//! process. What it must not carry is a decision: whether to start is re-checked here, so
//! a plan built a minute ago cannot start a second instance on a home that just got one.

use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_formproc::lifecycle::{self, Form, StartOutcome, StopOutcome};
use o824_plan::Step;

/// Runs one process step against `data_root`'s state.
pub fn run(data_root: &Path, install_id: &str, step: &Step) -> Result<(), Error> {
    match step.kind.as_str() {
        o824_plan::KIND_START_WEB => start(data_root, install_id, step, Form::Web),
        o824_plan::KIND_START_DESKTOP => start(data_root, install_id, step, Form::Desktop),
        o824_plan::KIND_STOP_WEB | o824_plan::KIND_STOP_DESKTOP => stop(data_root, step),
        other => Err(Error::new(
            Code::Internal,
            format!("apply: unknown process step kind {other:?}"),
        )),
    }
}

fn start(data_root: &Path, install_id: &str, step: &Step, form: Form) -> Result<(), Error> {
    let home = path_argument(step, o824_plan::ARG_HOME).ok_or_else(|| {
        Error::new(
            Code::Internal,
            format!("apply: step {:?} carries no home", step.kind),
        )
    })?;
    let port: Option<u16> = value_argument(step, o824_plan::ARG_PORT)
        .and_then(|value| value.parse().ok())
        .filter(|port| *port != 0);
    let executable = PathBuf::from(&step.path);

    let state_path = data_root.join(o824_state::STATE_FILE_NAME);
    let mut state = o824_state::load(&state_path)?;
    if let Some(existing) = lifecycle::blocking_record(&state, &home) {
        return Err(Error::new(
            Code::ConcurrentInstance,
            format!(
                "apply: an instance for this DSH home is already running ({} pid {}); stop it first",
                existing.install_id, existing.pid
            ),
        )
        .with_param("pid", existing.pid));
    }
    state
        .processes
        .retain(|_, record| lifecycle::is_alive(record));

    let outcome = lifecycle::start(form, &executable, &home, data_root, port, &mut |record| {
        // Written before the readiness wait: if this process dies while waiting, the
        // record is the only thing that can still stop the server.
        let mut record = record.clone();
        record.install_id = install_id.to_owned();
        let mut interim = state.clone();
        interim.remember_process(record);
        o824_state::save(&state_path, &interim)
    })?;

    // A desktop the user started themselves is running already; nothing was spawned, so
    // nothing is recorded. Writing a record for a pid we did not create would let `stop` kill
    // a process o824 does not own.
    let StartOutcome::Started(started) = outcome else {
        return Ok(());
    };

    let mut record = started.record.clone();
    record.install_id = install_id.to_owned();
    state.remember_process(record);
    o824_state::save(&state_path, &state)?;
    Ok(())
}

fn stop(data_root: &Path, step: &Step) -> Result<(), Error> {
    let install_id = value_argument(step, o824_plan::ARG_INSTALL_ID).ok_or_else(|| {
        Error::new(
            Code::Internal,
            format!("apply: step {:?} names no installation", step.kind),
        )
    })?;
    let state_path = data_root.join(o824_state::STATE_FILE_NAME);
    let mut state = o824_state::load(&state_path)?;

    match lifecycle::stop(&mut state, &install_id)? {
        StopOutcome::Stopped(pid) | StopOutcome::AlreadyStopped(pid) => {
            o824_state::save(&state_path, &state)?;
            let _ = pid;
            Ok(())
        }
        StopOutcome::Refused(pid, reason) => {
            // The record stays: it is the only evidence this kernel started something
            // whose pid now belongs to a stranger.
            Err(Error::new(Code::StateDrift, format!("apply: {reason}")).with_param("pid", pid))
        }
    }
}

/// The value of a `prefix=` argument.
fn value_argument(step: &Step, prefix: &str) -> Option<String> {
    step.args
        .iter()
        .find_map(|argument| argument.strip_prefix(prefix).map(str::to_owned))
}

/// The path value of a `prefix=` argument.
fn path_argument(step: &Step, prefix: &str) -> Option<PathBuf> {
    value_argument(step, prefix).map(PathBuf::from)
}
