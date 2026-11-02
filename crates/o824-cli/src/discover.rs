//! The `discover` subcommand.
//!
//! It lists the installations found in the uninstall registry and in per-user
//! install directories on every fixed volume. A damaged installation is reported
//! with health `"damaged"` and still exits 0; diagnosing it is `doctor`'s job.

use std::io::Write;
use std::path::PathBuf;

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_OK};

/// The help text for the discover subcommand.
pub const USAGE: &str = "\
Usage:
  o824 discover [--json] [--root <path>] [--home <path>] [--install <id>]

Lists the DeepSeek Harness installations found in the uninstall registry and in
per-user install directories on every fixed volume. Profiles are read from the
DSH home (<home>\\profiles), not from the install root.

Flags:
  --json            print the installation array and nothing else
  --root <path>     inspect this directory instead of scanning the machine
  --home <path>     DSH home to read profiles from (default: O824_HOME, then
                    DSH_HOME, then %USERPROFILE%\\.dsh; --root implies --home)
  --install <id>    select one install id (with --root: label the result)

--root and --install may be combined: --root supplies the path and --install
supplies the id. A damaged installation is reported with health \"damaged\" and
still exits 0; diagnosing it is doctor's job.
";

/// Runs `o824 discover`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("install"),
    ]);
    if let Err(error) = flags.parse("discover", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 discover: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let explicit_root = flags.value("root").filter(|root| !root.is_empty());
    let mut options = o824_discovery::Options {
        roots: None,
        extra_dirs: Vec::new(),
        home: flags.value("home").map(str::to_owned),
        ..Default::default()
    };
    if let Some(root) = explicit_root {
        options.roots = Some(vec![PathBuf::from(root)]);
        // A fixture tree used through --root carries its own profiles\, so the
        // root doubles as the DSH home unless --home overrides it.
        if options.home.as_deref().unwrap_or("").is_empty() {
            options.home = Some(root.to_owned());
        }
    }

    let mut installs = match o824_discovery::discover(&options) {
        Ok(installs) => installs,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    if let Some(install_id) = flags.value("install").filter(|id| !id.is_empty()) {
        if explicit_root.is_some() && installs.len() == 1 {
            // --root supplies the path and --install supplies the id; the two are
            // complementary, not conflicting.
            installs[0].install_id = install_id.to_owned();
        } else {
            installs.retain(|install| install.install_id == install_id);
            if installs.is_empty() {
                let error = Error::new(
                    Code::Usage,
                    format!("o824 discover: no installation with id {install_id:?}"),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        }
    }

    if flags.flag("json") {
        let Ok(text) = serde_json::to_string(&installs) else {
            let error = Error::new(
                Code::Internal,
                "o824 discover: cannot encode the installation list",
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        };
        if let Err(error) = writeln!(stdout, "{text}") {
            let error = Error::new(
                Code::Internal,
                format!("o824 discover: cannot write the installation list: {error}"),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
        return EXIT_OK;
    }

    if let Some(note) = orphan_note(&options, &installs) {
        let _ = writeln!(stderr, "{note}");
    }

    let table = install_table(&installs);
    if let Err(error) = stdout.write_all(table.as_bytes()) {
        let error = Error::new(
            Code::Internal,
            format!("o824 discover: cannot write the installation table: {error}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    EXIT_OK
}

/// The note shown when a DSH home has profiles but nothing installed uses them.
///
/// A per-install check cannot report this: there is no installation to attach the
/// finding to. It is the one state where "profiles exist" is the only evidence left.
fn orphan_note(
    options: &o824_discovery::Options,
    installs: &[o824_model::Install],
) -> Option<String> {
    let home = o824_discovery::resolve_home(options.home.as_deref());
    let orphans = o824_discovery::orphan_profiles(&home, installs);
    if orphans.is_empty() {
        return None;
    }
    Some(format!(
        "no installation found, but {} holds profiles: {}",
        home.display(),
        orphans.join(", ")
    ))
}

/// Renders the human-facing table: a header plus one row per installation,
/// aligned with spaces and free of colour.
///
/// The columns are padded to the widest cell plus two spaces, which is what the
/// reference implementation's tab writer emits for this shape.
fn install_table(installs: &[o824_model::Install]) -> String {
    let mut rows: Vec<[&str; 5]> = vec![["id", "kind", "version", "root", "health"]];
    for install in installs {
        rows.push([
            install.install_id.as_str(),
            install.kind.as_str(),
            install.version.as_str(),
            install.root.as_str(),
            install.health.as_str(),
        ]);
    }

    let mut widths = [0usize; 5];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }

    let mut table = String::new();
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            table.push_str(cell);
            if index + 1 < row.len() {
                let padding = widths[index] - cell.chars().count() + 2;
                table.push_str(&" ".repeat(padding));
            }
        }
        table.push('\n');
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_model::Install;

    fn install(id: &str, root: &str) -> Install {
        Install {
            install_id: id.to_owned(),
            kind: "desktop".to_owned(),
            root: root.to_owned(),
            version: "0.2.1-rc.3".to_owned(),
            profiles: vec!["desktop".to_owned()],
            health: "ok".to_owned(),
            home: r"C:\home".to_owned(),
        }
    }

    #[test]
    fn the_table_pads_every_column_by_two_spaces() {
        let table = install_table(&[install("i1", r"C:\a")]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines[0], "id  kind     version     root  health");
        assert_eq!(lines[1], r"i1  desktop  0.2.1-rc.3  C:\a  ok");
    }

    #[test]
    fn an_empty_list_still_prints_the_header() {
        let table = install_table(&[]);
        assert_eq!(table, "id  kind  version  root  health\n");
    }

    #[test]
    fn the_orphan_note_names_the_home_and_its_profiles() {
        let home = o824_fixtures::TempDir::new("o824-cli-orphan-").expect("temp dir");
        std::fs::create_dir_all(home.path().join("profiles").join("desktop")).expect("mkdir");
        let options = o824_discovery::Options {
            roots: None,
            extra_dirs: Vec::new(),
            home: Some(home.path().display().to_string()),
            ..Default::default()
        };
        let note = orphan_note(&options, &[]).expect("a note");
        assert!(note.contains("desktop"), "{note}");
        assert!(note.contains(&home.path().display().to_string()), "{note}");
    }

    #[test]
    fn no_orphan_note_when_an_installation_exists() {
        let home = o824_fixtures::TempDir::new("o824-cli-orphan-").expect("temp dir");
        std::fs::create_dir_all(home.path().join("profiles").join("desktop")).expect("mkdir");
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let install =
            o824_discovery::validate_with(fixture.root(), home.path(), None).expect("validate");
        let options = o824_discovery::Options {
            roots: None,
            extra_dirs: Vec::new(),
            home: Some(home.path().display().to_string()),
            ..Default::default()
        };
        assert!(orphan_note(&options, &[install]).is_none());
    }
}
