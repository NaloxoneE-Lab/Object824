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

    commit_outcome(&state_path, state, outcome, install_id)
}

/// Writes what a start produced — including the case where it produced nothing.
///
/// A desktop the user started themselves is running already; `apply` spawned nothing, so
/// nothing is recorded: a record for a pid we did not create would let `stop` kill a process
/// o824 does not own. What *is* written is the pruned state: dead records were dropped above,
/// and leaving them on disk is how a stale pid came to be reported as "just started".
fn commit_outcome(
    state_path: &Path,
    mut state: o824_state::State,
    outcome: StartOutcome,
    install_id: &str,
) -> Result<(), Error> {
    match outcome {
        StartOutcome::Started(started) => {
            let mut record = started.record.clone();
            record.install_id = install_id.to_owned();
            state.remember_process(record);
        }
        StartOutcome::AlreadyRunning(_) => {}
    }
    o824_state::save(state_path, &state)
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

#[cfg(test)]
mod tests {
    use super::*;
    use o824_state::ProcessRecord;

    /// A record whose pid cannot be alive: pid 0 is never a user process.
    fn dead_record(install_id: &str, home: &Path) -> ProcessRecord {
        ProcessRecord {
            install_id: install_id.to_owned(),
            pid: 0,
            port: None,
            home: home.display().to_string(),
            image: String::new(),
            created: 1,
            started_at: "2026-11-06T00:00:00Z".to_owned(),
            log_path: String::new(),
        }
    }

    /// The 2026-11-06 second defect: a start that found the desktop already running wrote
    /// nothing, so the stale record of an *earlier failed* start stayed on disk and the CLI
    /// reported its pid as the thing that had just started.
    #[test]
    fn the_stale_record_is_dropped_even_when_nothing_was_started() {
        let temp = o824_fixtures::TempDir::new("o824-commit-").expect("temp dir");
        let state_path = temp.path().join(o824_state::STATE_FILE_NAME);
        let mut state = o824_state::State::default();
        state.remember_process(dead_record("i3", temp.path()));
        o824_state::save(&state_path, &state).expect("seed the stale record");

        // `apply` prunes dead records before it starts anything; this is the state it
        // would hand over.
        let mut pruned = o824_state::load(&state_path).expect("load");
        pruned
            .processes
            .retain(|_, record| lifecycle::is_alive(record));
        assert!(
            pruned.processes.is_empty(),
            "the record named pid 0, which is never alive"
        );

        commit_outcome(
            &state_path,
            pruned,
            StartOutcome::AlreadyRunning(4242),
            "i3",
        )
        .expect("commit");

        let after = o824_state::load(&state_path).expect("load after");
        assert!(
            !after.processes.contains_key("i3"),
            "a start that spawned nothing must not leave a record behind: {:?}",
            after.processes
        );
    }

    /// And a start that did spawn something still records it.
    #[test]
    fn a_real_start_is_still_recorded() {
        let temp = o824_fixtures::TempDir::new("o824-commit-").expect("temp dir");
        let state_path = temp.path().join(o824_state::STATE_FILE_NAME);
        let state = o824_state::State::default();

        let record = ProcessRecord {
            install_id: String::new(),
            pid: std::process::id(),
            port: Some(3000),
            home: temp.path().display().to_string(),
            image: String::new(),
            created: 1,
            started_at: "2026-11-06T00:00:00Z".to_owned(),
            log_path: String::new(),
        };
        let started = lifecycle::Started {
            pid: record.pid,
            url: None,
            port: record.port,
            log_path: PathBuf::from(&record.log_path),
            record,
        };
        commit_outcome(&state_path, state, StartOutcome::Started(started), "web").expect("commit");

        let after = o824_state::load(&state_path).expect("load after");
        let written = after.processes.get("web").expect("the record is there");
        assert_eq!(written.pid, std::process::id());
        assert_eq!(written.install_id, "web");
    }
}
