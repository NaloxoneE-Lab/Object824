//! The apply journal: one JSON file per plan under `<dataRoot>\journal`.
//!
//! It is written before the first mutation, so an interrupted apply is
//! diagnosable: a journal whose state is neither `done` nor `failed` is exactly
//! what F20 reports. `finishedAt` and the two error fields are absent while the
//! run is in flight, and `rollbackError` is filled only when the rollback itself
//! failed — a `failed` journal with an empty `rollbackError` means the tree is
//! back where it started.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::Timestamp;
use o824_plan::Step;
use serde::{Deserialize, Serialize};

/// The directory under the data root holding one file per plan.
pub const JOURNAL_DIR_NAME: &str = "journal";
/// The only extension a journal file can have.
pub const JOURNAL_FILE_EXT: &str = ".json";

/// The state of one apply run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The run is in flight.
    Applying,
    /// Every step ran.
    Done,
    /// The run stopped and rolled back.
    Failed,
}

impl State {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            State::Applying => "applying",
            State::Done => "done",
            State::Failed => "failed",
        }
    }
}

/// One apply run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    /// The journal id (the plan id).
    pub id: String,
    /// The plan the run executed.
    pub plan_id: String,
    /// The installation the plan repaired.
    pub install_id: String,
    /// The absolute scope root.
    pub root: String,
    /// The run's state.
    pub state: State,
    /// When the run started.
    pub started_at: Timestamp,
    /// When it finished; absent while it is in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// The steps, with the notes apply recorded as it ran them.
    pub steps: Vec<Step>,
    /// The failure the run reported; absent on success.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// The rollback's own failure; absent when the rollback succeeded.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rollback_error: String,
}

impl Journal {
    /// A journal for a run that is about to start.
    pub fn new(
        plan_id: &str,
        install_id: &str,
        root: &Path,
        started_at: Timestamp,
        steps: Vec<Step>,
    ) -> Journal {
        Journal {
            id: plan_id.to_owned(),
            plan_id: plan_id.to_owned(),
            install_id: install_id.to_owned(),
            root: root.display().to_string(),
            state: State::Applying,
            started_at,
            finished_at: None,
            steps,
            error: String::new(),
            rollback_error: String::new(),
        }
    }
}

/// Writes the journal for `plan_id` atomically.
pub fn write(data_root: &Path, plan_id: &str, journal: &Journal) -> Result<(), Error> {
    let path = journal_path(data_root, plan_id)?;
    let mut data = serde_json::to_vec_pretty(journal).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("apply: cannot encode the journal for plan {plan_id}: {error}"),
        )
    })?;
    data.push(b'\n');

    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)
            .map_err(|error| file_error("create the journal directory", directory, &error))?;
    }
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let written = (|| -> io::Result<()> {
        fs::write(&temporary, &data)?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(file_error("write the journal", &path, &error));
    }
    Ok(())
}

/// Resolves `<dataRoot>\journal\<planID>.json`, refusing an id that is not
/// exactly one path element: the id comes from the plan, and a hand-edited one
/// must never escape the journal directory.
pub fn journal_path(data_root: &Path, plan_id: &str) -> Result<PathBuf, Error> {
    if plan_id.is_empty()
        || plan_id == "."
        || plan_id == ".."
        || plan_id.contains(['/', '\\', ':', '\0'])
    {
        return Err(Error::new(
            Code::Internal,
            format!("apply: {plan_id:?} is not a usable plan id"),
        ));
    }
    Ok(data_root
        .join(JOURNAL_DIR_NAME)
        .join(format!("{plan_id}{JOURNAL_FILE_EXT}")))
}

fn file_error(operation: &str, path: &Path, cause: &io::Error) -> Error {
    let code = if cause.kind() == io::ErrorKind::PermissionDenied {
        Code::Permission
    } else {
        Code::Internal
    };
    Error::new(
        code,
        format!(
            "apply: cannot {operation} {:?}: {cause}",
            path.display().to_string()
        ),
    )
    .with_param("error", cause.to_string())
    .with_param("path", path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    #[test]
    fn a_journal_round_trips_with_its_documented_field_names() {
        let store = TempDir::new("o824-journal-").expect("store");
        let journal = Journal::new(
            "plan-20261101T121041Z-abc12345",
            "i1",
            Path::new(r"C:\harness"),
            Timestamp::parse("2026-11-01T12:10:41Z").expect("timestamp"),
            Vec::new(),
        );
        write(store.path(), &journal.id, &journal).expect("write");

        let path = journal_path(store.path(), &journal.id).expect("path");
        let text = fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("\"planId\": \"plan-20261101T121041Z-abc12345\""),
            "{text}"
        );
        assert!(text.contains("\"state\": \"applying\""), "{text}");
        assert!(text.contains("\"installId\": \"i1\""), "{text}");
        // An in-flight journal has no finish time and no errors.
        assert!(!text.contains("finishedAt"), "{text}");
        assert!(!text.contains("rollbackError"), "{text}");

        let restored: Journal = serde_json::from_str(&text).expect("parse");
        assert_eq!(restored, journal);
    }

    /// A journal whose state is neither done nor failed is exactly what F20
    /// reports, so the spelling matters.
    #[test]
    fn the_three_states_have_the_documented_spellings() {
        assert_eq!(State::Applying.as_str(), "applying");
        assert_eq!(State::Done.as_str(), "done");
        assert_eq!(State::Failed.as_str(), "failed");
        assert_eq!(
            serde_json::to_string(&State::Done).expect("serialize"),
            "\"done\""
        );
    }

    #[test]
    fn an_id_that_could_escape_the_journal_directory_is_refused() {
        let store = TempDir::new("o824-journal-").expect("store");
        for bad in ["", ".", "..", "a/b", "a\\b", "c:stream"] {
            let error = journal_path(store.path(), bad).expect_err("must fail");
            assert_eq!(error.code, Code::Internal, "{bad:?}");
        }
    }
}
