//! F20: an install that was interrupted and never finished.
//!
//! A journal records one install attempt and its state. A state of `done` or
//! `failed` is terminal and healthy; any other state — `applying`, `verifying`,
//! or a state that cannot be read at all — means an install was interrupted,
//! which leaves the tree in an unknown condition. F20 is fixability B: an
//! interrupted install may have left any subset of its files behind, so it is
//! never repaired by rewriting one thing, which is why its repair is `manual`.

use std::fs;
use std::path::Path;

use o824_errs::Code;
use o824_model::{Category, Finding, Repair, Severity};
use serde_json::Value;

use crate::{params, sort_findings, PARAM_JOURNAL, PARAM_STATE, REPAIR_KIND_MANUAL};

/// F20: an install that was interrupted and never finished.
pub const CHECK_HALF_INSTALL: &str = "F20";

const MESSAGE_HALF_INSTALL: &str = "An install was interrupted and never finished";
/// Asks plan to roll the install back or reinstall it.
const PLAN_HINT_ROLLBACK_OR_REINSTALL: &str = "rollback-or-reinstall";

/// Holds one file per recorded install, under the data root.
pub const JOURNAL_DIR_NAME: &str = "journal";
/// The only extension a journal file can have.
const JOURNAL_FILE_EXT: &str = ".json";

/// The two terminal states: the install either ran to completion or stopped and
/// recorded the failure.
const JOURNAL_STATE_DONE: &str = "done";
const JOURNAL_STATE_FAILED: &str = "failed";

/// Returns the F20 findings for the journals under `<dataRoot>\journal`.
///
/// A missing journal directory is not a fault: it means no install was ever
/// recorded through the journal, not that one was interrupted.
pub fn half_install(data_root: &Path) -> Vec<Finding> {
    let dir = data_root.join(JOURNAL_DIR_NAME);
    let Ok(entries) = fs::read_dir(&dir) else {
        // No journal directory, or one that cannot be listed: there is no
        // journal to judge, and an unreadable directory is not an interrupted
        // install.
        return Vec::new();
    };

    let mut journals: Vec<fs::DirEntry> = entries.flatten().collect();
    // `ReadDir` sorts by name in the reference implementation, so the findings
    // are already in a stable order there.
    journals.sort_by_key(|entry| entry.file_name());

    let mut findings = Vec::new();
    for entry in journals {
        let path = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.to_lowercase().ends_with(JOURNAL_FILE_EXT) {
            continue;
        }
        let state = journal_state(&path);
        if is_finished_journal_state(&state) {
            continue;
        }
        findings.push(half_install_finding(&name, &state));
    }

    sort_findings(&mut findings);
    findings
}

/// Returns the state one journal records, trimmed, or `""` when the file cannot
/// be read or parsed.
fn journal_state(path: &Path) -> String {
    let Ok(data) = fs::read(path) else {
        return String::new();
    };
    #[derive(serde::Deserialize)]
    struct Journal {
        #[serde(default)]
        state: String,
    }
    match serde_json::from_slice::<Journal>(&data) {
        Ok(journal) => journal.state.trim().to_owned(),
        Err(_) => String::new(),
    }
}

/// True when `state` is one of the two terminal states.
///
/// The comparison ignores case because a hand-edited journal may shout.
fn is_finished_journal_state(state: &str) -> bool {
    matches!(
        state.to_lowercase().as_str(),
        JOURNAL_STATE_DONE | JOURNAL_STATE_FAILED
    )
}

/// Builds one F20 finding for an unfinished journal.
fn half_install_finding(journal: &str, state: &str) -> Finding {
    Finding::new(
        CHECK_HALF_INSTALL,
        Severity::Error,
        Code::HalfInstall,
        Category::Install,
    )
    .with_params(params(&[
        (PARAM_JOURNAL, Value::from(journal)),
        (PARAM_STATE, Value::from(state)),
    ]))
    .with_fallback_message(MESSAGE_HALF_INSTALL)
    .with_repair(Repair {
        kind: REPAIR_KIND_MANUAL.to_owned(),
        plan_hint: PLAN_HINT_ROLLBACK_OR_REINSTALL.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(dir).expect("mkdir");
        fs::write(dir.join(name), body).expect("write");
    }

    #[test]
    fn the_half_install_fixture_is_reported() {
        let fixture = o824_fixtures::build("half-install").expect("build");
        let findings = half_install(fixture.root());
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].params[PARAM_JOURNAL],
            serde_json::json!("install-1.json")
        );
        assert_eq!(
            findings[0].params[PARAM_STATE],
            serde_json::json!("applying")
        );
    }

    #[test]
    fn a_missing_journal_directory_is_not_a_fault() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        assert!(half_install(dir.path()).is_empty());
    }

    #[test]
    fn terminal_states_are_healthy() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let journal = dir.path().join(JOURNAL_DIR_NAME);
        write(&journal, "a.json", r#"{"state":"done"}"#);
        write(&journal, "b.json", r#"{"state":"FAILED"}"#);
        assert!(
            half_install(dir.path()).is_empty(),
            "{:?}",
            half_install(dir.path())
        );
    }

    #[test]
    fn a_journal_that_cannot_be_read_is_an_interrupted_install() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let journal = dir.path().join(JOURNAL_DIR_NAME);
        write(&journal, "broken.json", "{not json");
        let findings = half_install(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].params[PARAM_STATE], serde_json::json!(""));
    }

    #[test]
    fn non_json_files_are_ignored() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let journal = dir.path().join(JOURNAL_DIR_NAME);
        write(&journal, "notes.txt", "applying");
        assert!(half_install(dir.path()).is_empty());
    }
}
