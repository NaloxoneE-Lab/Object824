//! The `snapshot` subcommand: create and list the settings snapshots that back
//! every repair.
//!
//! A snapshot copies each profile's `package.json`, `cordis.yml`,
//! `cordis.patch.yml`, `pnpm-lock.yaml` and `pnpm-workspace.yaml`, plus the
//! install manifest (`<root>\version`), under
//! `<data root>\snapshots\<install id>\<snapshot id>\`. Every copy is verified
//! against its source's sha256 before `meta.json` marks the snapshot complete;
//! `node_modules` is never captured. The newest five snapshots per installation
//! are kept.

use std::io::Write;
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::Install;

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_OK};

/// The help text for the snapshot subcommand.
pub const USAGE: &str = "\
Usage:
  o824 snapshot create --install <id> [--if-changed] [--json]
  o824 snapshot create --root <path> [--install <id>] [--home <path>] [--if-changed] [--json]
  o824 snapshot list --install <id> [--json]
  o824 snapshot list --root <path> [--install <id>] [--home <path>] [--json]

Writes or lists the settings snapshots that back every repair.

Flags:
  --install <id>    installation id; also the key of its snapshot store
  --root <path>     snapshot this installation root instead of discovering it;
                    with list, derive the same local-<hash> store id create used
  --home <path>     DSH home used to resolve an installation (--root implies
                    --home)
  --if-changed      create only when the captured settings differ from the
                    newest snapshot (create only)
  --json            print JSON and nothing else

create prints the snapshot id and the directory that holds it; with --json it
prints the Snapshot document. list prints \"<id>  <createdAt>  <n> files\" per
line, newest first, or the Snapshot array with --json.
";

/// Runs `o824 snapshot`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    run_with(args, stdout, stderr, None)
}

/// Runs `o824 snapshot` with an explicit data root, for tests.
pub fn run_with(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&Path>,
) -> i32 {
    let Some(subcommand) = args.first() else {
        let error = Error::new(
            Code::Usage,
            "o824 snapshot: pass a subcommand: create or list",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };
    match subcommand.as_str() {
        "create" => create(&args[1..], stdout, stderr, data_root),
        "list" => list(&args[1..], stdout, stderr, data_root),
        "-h" | "--help" | "-help" | "help" => {
            let _ = stdout.write_all(USAGE.as_bytes());
            EXIT_OK
        }
        other => {
            let error = Error::new(
                Code::Usage,
                format!("o824 snapshot: unknown subcommand {other:?} (want create or list)"),
            );
            let _ = writeln!(stderr, "{error}");
            exit_code_of(Some(&error))
        }
    }
}

/// `snapshot create`.
fn create(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&Path>,
) -> i32 {
    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::boolean("if-changed"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("install"),
    ]);
    if let Err(error) = flags.parse("snapshot create", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 snapshot create: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let install = match crate::install::select(
        "snapshot",
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
    let data_root = resolve_data_root(data_root);

    // Snapshotting writes shared state, so it takes the machine lock every other
    // write operation takes.
    let mut handle = match o824_lock::acquire(&data_root) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    let scope = scope_of(&install);
    let (snapshot, changed) = if flags.flag("if-changed") {
        match o824_snapshot::create_if_changed(&data_root, &install.install_id, &scope) {
            Ok((snapshot, changed)) => (snapshot, changed),
            Err(error) => {
                let _ = handle.release();
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        }
    } else {
        match o824_snapshot::create(&data_root, &install.install_id, &scope) {
            Ok(snapshot) => (snapshot, true),
            Err(error) => {
                let _ = handle.release();
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        }
    };
    let release = handle.release();
    if let Err(error) = release {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    if flags.flag("json") {
        return write_json(stdout, stderr, &snapshot);
    }
    if changed {
        let _ = writeln!(
            stdout,
            "snapshot {} saved for install {}",
            snapshot.id, snapshot.install_id
        );
    } else {
        let _ = writeln!(
            stdout,
            "snapshot {} unchanged for install {}",
            snapshot.id, snapshot.install_id
        );
    }
    EXIT_OK
}

/// `snapshot list`.
fn list(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&Path>,
) -> i32 {
    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("install"),
    ]);
    if let Err(error) = flags.parse("snapshot list", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 snapshot list: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let install = match crate::install::select(
        "snapshot",
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
    let data_root = resolve_data_root(data_root);

    let snapshots = match o824_snapshot::list(&data_root, &install.install_id) {
        Ok(snapshots) => snapshots,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    if flags.flag("json") {
        return write_json(stdout, stderr, &snapshots);
    }
    for snapshot in &snapshots {
        let _ = writeln!(
            stdout,
            "{}  {}  {} files",
            snapshot.id,
            snapshot.created_at.to_rfc3339_seconds(),
            snapshot.files.len()
        );
    }
    EXIT_OK
}

/// The scope root a snapshot covers: the DSH home the profiles live under.
fn scope_of(install: &Install) -> PathBuf {
    if install.home.trim().is_empty() {
        PathBuf::from(&install.root)
    } else {
        PathBuf::from(&install.home)
    }
}

fn resolve_data_root(explicit: Option<&Path>) -> PathBuf {
    match explicit {
        Some(root) => root.to_path_buf(),
        None => o824_state::data_root(None),
    }
}

fn write_json<T: serde::Serialize>(
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    value: &T,
) -> i32 {
    let text = match serde_json::to_string(value) {
        Ok(text) => text,
        Err(error) => {
            let error = Error::new(
                Code::Internal,
                format!("o824 snapshot: cannot encode the snapshot document: {error}"),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };
    if let Err(error) = writeln!(stdout, "{text}") {
        let error = Error::new(
            Code::Internal,
            format!("o824 snapshot: cannot write the snapshot document: {error}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    EXIT_OK
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
    fn create_then_list_round_trips() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-snapshot-cli-").expect("store");
        let root = fixture.root().display().to_string();

        let (code, stdout, stderr) = run_capture(&["create", "--root", &root], store.path());
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("saved for install"), "{stdout}");

        let (code, stdout, stderr) = run_capture(&["list", "--root", &root], store.path());
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("files"), "{stdout}");
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
    }

    #[test]
    fn if_changed_writes_once() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-snapshot-cli-").expect("store");
        let root = fixture.root().display().to_string();

        let (_, first, _) = run_capture(&["create", "--root", &root, "--if-changed"], store.path());
        assert!(first.contains("saved"), "{first}");
        let (_, second, _) =
            run_capture(&["create", "--root", &root, "--if-changed"], store.path());
        assert!(second.contains("unchanged"), "{second}");
    }

    #[test]
    fn an_unknown_subcommand_is_a_usage_error() {
        let store = TempDir::new("o824-snapshot-cli-").expect("store");
        let (code, _, stderr) = run_capture(&["frobnicate"], store.path());
        assert_eq!(code, crate::EXIT_USAGE);
        assert!(stderr.contains("unknown subcommand"), "{stderr}");
    }
}
