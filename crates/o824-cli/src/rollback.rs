//! The `rollback` subcommand: restore one installation from a snapshot.
//!
//! It is a write operation, so it is deliberate: it takes the machine lock,
//! restores exactly one snapshot (`--latest` or `--to <id>`, never both and never
//! neither), proves every restored file against the sha256 recorded when the
//! snapshot was taken, and records what it did in the journal so an interrupted
//! or failed rollback is diagnosable.

use std::io::Write;
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::{Install, Timestamp};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_OK};

/// The help text for the rollback subcommand.
pub const USAGE: &str = "\
Usage:
  o824 rollback --install <id> (--latest | --to <snapshot>) [--json]
  o824 rollback --root <path> [--install <id>] [--home <path>] (--latest | --to <snapshot>) [--json]

Restores one installation from a snapshot taken under <data root>\\snapshots\\.

Flags:
  --install <id>  installation id; also the key of its snapshot store
  --root <path>   restore this installation root instead of discovering it
  --home <path>   DSH home used to resolve an installation (--root implies
                  --home; this flag overrides that)
  --latest        restore the newest snapshot
  --to <id>       restore exactly this snapshot id
  --json          print the restored Snapshot document and nothing else

Exit code 0 means the snapshot was restored, 2 means the command line was
unusable, 5 means the installation has no such snapshot or the restore could not
prove it worked, and 7 means another O824 process holds the lock.
";

/// Runs `o824 rollback`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    run_with(args, stdout, stderr, None)
}

/// Runs `o824 rollback` with an explicit data root, for tests.
pub fn run_with(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&Path>,
) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("latest"),
        FlagSpec::boolean("json"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("install"),
        FlagSpec::valued("to"),
    ]);
    if let Err(error) = flags.parse("rollback", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 rollback: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let latest = flags.flag("latest");
    let target = flags.value("to").filter(|id| !id.is_empty());
    let target = match (latest, target) {
        (true, Some(_)) => {
            let error = Error::new(
                Code::Usage,
                "o824 rollback: pass either --latest or --to <snapshot>, not both",
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
        (false, None) => {
            let error = Error::new(
                Code::Usage,
                "o824 rollback: pass --latest or --to <snapshot>",
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
        (true, None) => None,
        (false, Some(id)) => Some(id.to_owned()),
    };

    let install = match crate::install::select(
        "rollback",
        flags.value("install"),
        flags.value("root"),
        flags.value("home"),
    ) {
        Ok(install) => install,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    let data_root = match data_root {
        Some(root) => root.to_path_buf(),
        None => o824_state::data_root(None),
    };

    let snapshots = match o824_snapshot::list(&data_root, &install.install_id) {
        Ok(snapshots) => snapshots,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    let snapshot = match target {
        None => match snapshots.first() {
            Some(snapshot) => snapshot.clone(),
            None => {
                let error = Error::new(
                    Code::StateDrift,
                    format!(
                        "rollback: installation {:?} has no snapshot",
                        install.install_id
                    ),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        },
        Some(id) => match snapshots.into_iter().find(|snapshot| snapshot.id == id) {
            Some(snapshot) => snapshot,
            None => {
                let error = Error::new(
                    Code::StateDrift,
                    format!(
                        "rollback: installation {:?} has no snapshot {:?}",
                        install.install_id, id
                    ),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        },
    };

    let mut handle = match o824_lock::acquire(&data_root) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    // The journal is written before the copy, so a rollback killed mid-restore is
    // an unfinished journal and the next doctor reports it as F20.
    let journal_id = journal_id(Timestamp::now());
    let mut entry = o824_apply::journal::Journal::new(
        &journal_id,
        &install.install_id,
        &scope_of(&install),
        Timestamp::now(),
        Vec::new(),
    );
    entry.root = install.root.clone();
    if let Err(error) = o824_apply::journal::write(&data_root, &journal_id, &entry) {
        let _ = handle.release();
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let mut restore_error: Option<Error> = None;
    // A snapshot covers the DSH home, not the application root: restoring into the
    // application directory would write profiles\ where they do not belong.
    let scope = scope_of(&install);
    let restored = o824_snapshot::restore(&data_root, &snapshot, &scope);
    match &restored {
        Ok(()) => {
            entry.state = o824_apply::journal::State::Done;
            entry.finished_at = Some(Timestamp::now());
        }
        Err(error) => {
            entry.state = o824_apply::journal::State::Failed;
            entry.finished_at = Some(Timestamp::now());
            entry.error = error.to_string();
            restore_error = Some(error.clone());
        }
    }
    let _ = o824_apply::journal::write(&data_root, &journal_id, &entry);
    let _ = handle.release();

    match restored {
        Ok(()) => {
            if flags.flag("json") {
                match serde_json::to_string(&snapshot) {
                    Ok(text) => {
                        let _ = writeln!(stdout, "{text}");
                        EXIT_OK
                    }
                    Err(error) => {
                        let error = Error::new(
                            Code::Internal,
                            format!("o824 rollback: cannot encode the snapshot: {error}"),
                        );
                        let _ = writeln!(stderr, "{error}");
                        exit_code_of(Some(&error))
                    }
                }
            } else {
                let _ = writeln!(
                    stdout,
                    "restored snapshot {} for install {}",
                    snapshot.id, snapshot.install_id
                );
                EXIT_OK
            }
        }
        Err(_) => match restore_error {
            Some(error) => {
                let _ = writeln!(stderr, "{error}");
                exit_code_of(Some(&error))
            }
            None => {
                let error = Error::new(Code::Internal, "rollback: the restore failed silently");
                let _ = writeln!(stderr, "{error}");
                exit_code_of(Some(&error))
            }
        },
    }
}

/// The scope root a snapshot covers: the DSH home the profiles live under.
fn scope_of(install: &Install) -> PathBuf {
    if install.home.trim().is_empty() {
        PathBuf::from(&install.root)
    } else {
        PathBuf::from(&install.home)
    }
}

/// A journal id for one rollback: the UTC timestamp without colons, so it is a
/// valid Windows file name.
fn journal_id(now: Timestamp) -> String {
    format!(
        "rollback-{:04}{:02}{:02}T{:02}{:02}{:02}.{:09}Z",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanos()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn run_capture(args: &[&str], data_root: &Path) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run_with(&args, &mut stdout, &mut stderr, Some(data_root));
        (
            code,
            String::from_utf8(stdout).expect("utf8"),
            String::from_utf8(stderr).expect("utf8"),
        )
    }

    #[test]
    fn a_snapshot_can_be_restored() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-rollback-").expect("store");
        let root = fixture.root().display().to_string();

        let snapshot = o824_snapshot::create(store.path(), "i1", fixture.root()).expect("create");
        let patch = fixture.profile_dir().join("cordis.patch.yml");
        std::fs::write(&patch, "corrupted\n").expect("corrupt");

        let (code, stdout, stderr) = run_capture(
            &["--root", &root, "--install", "i1", "--latest"],
            store.path(),
        );
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("restored snapshot"), "{stdout}");
        assert_ne!(
            std::fs::read_to_string(&patch).expect("read"),
            "corrupted\n"
        );
        assert_eq!(snapshot.install_id, "i1");
    }

    #[test]
    fn neither_latest_nor_to_is_a_usage_error() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-rollback-").expect("store");
        let root = fixture.root().display().to_string();
        let (code, _, stderr) = run_capture(&["--root", &root], store.path());
        assert_eq!(code, crate::EXIT_USAGE);
        assert!(stderr.contains("--latest"), "{stderr}");
    }

    #[test]
    fn an_installation_without_a_snapshot_is_state_drift() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-rollback-").expect("store");
        let root = fixture.root().display().to_string();
        let (code, _, stderr) = run_capture(
            &["--root", &root, "--install", "i1", "--latest"],
            store.path(),
        );
        assert_eq!(code, crate::EXIT_STATE_DRIFT);
        assert!(stderr.contains("has no snapshot"), "{stderr}");
    }
}
