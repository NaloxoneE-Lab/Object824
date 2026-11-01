//! The `doctor` subcommand.
//!
//! Exit code 1 means findings were reported, which is a diagnosis rather than an
//! error: a UI switches on it and a script may too. Exit code 0 means the
//! installation is clean, and 2 means the command line was unusable.
//!
//! `doctor` is read-only: it writes nothing, and every check is a pure function
//! of the tree.

use std::io::Write;
use std::path::Path;

use o824_errs::{Code, Error};
use o824_model::Install;

use crate::flags::{FlagSet, FlagSpec};
use crate::render::write_findings;
use crate::{exit_code_of, EXIT_FINDINGS, EXIT_OK};

/// The help text for the doctor subcommand.
pub const USAGE: &str = "\
Usage:
  o824 doctor --install <id> [--profile <name>] [--json]
  o824 doctor --root <path> [--home <path>] [--profile <name>] [--json]

Diagnoses one DeepSeek Harness installation: the profile's plugin links and
bridges, its loader entries and profile files, the pnpm build whitelist, and any
install journal that was never finished. Every finding is printed as one line:

  F03  warning  E_DUP_LOADER  duplicates=1 ids=[dsh-echo]

Flags:
  --install <id>    diagnose one discovered installation, selected by id
  --root <path>     diagnose this directory instead of scanning the machine
  --home <path>     DSH home whose profiles are diagnosed (--root implies
                    --home; this flag overrides that)
  --profile <name>  diagnose only this profile, not the installation's list
  --json            print the findings envelope and nothing else

One of --install or --root is required: doctor never guesses which installation
to diagnose. Exit code 1 means findings were reported, 0 means the installation
is clean, and 2 means the command line was unusable.
";

/// Runs `o824 doctor`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    run_with(args, stdout, stderr, None)
}

/// Runs `o824 doctor` with an explicit data root.
///
/// The process-level entry point reads the data root from the environment; the
/// acceptance tests pass one in instead, because a test binary cannot set an
/// environment variable without racing every other test in the process.
pub fn run_with(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&Path>,
) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("profile"),
        FlagSpec::valued("install"),
    ]);
    if let Err(error) = flags.parse("doctor", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 doctor: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let install = match select_install(
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
    // --profile narrows the iteration to exactly the named profile; the checks
    // fall back to `desktop` only when the list is empty.
    let install = match flags.value("profile") {
        Some(profile) if !profile.is_empty() => Install {
            profiles: vec![profile.to_owned()],
            ..install
        },
        _ => install,
    };

    // The journal lives under the per-user data root, which O824_DATA_ROOT
    // overrides inside the state rule; doctor has no data-root flag of its own.
    let data_root = match data_root {
        Some(root) => root.to_path_buf(),
        None => o824_state::data_root(None),
    };
    let envelope = o824_verify::doctor(&install, &data_root);

    if flags.flag("json") {
        let Ok(text) = serde_json::to_string(&envelope) else {
            let error = Error::new(
                Code::Internal,
                "o824 doctor: cannot encode the findings envelope",
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        };
        if let Err(error) = writeln!(stdout, "{text}") {
            let error = Error::new(
                Code::Internal,
                format!("o824 doctor: cannot write the findings envelope: {error}"),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    } else if let Err(error) = write_findings(stdout, &envelope.findings) {
        let error = Error::new(
            Code::Internal,
            format!("o824 doctor: cannot write findings: {error}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    if envelope.findings.is_empty() {
        EXIT_OK
    } else {
        EXIT_FINDINGS
    }
}

/// Resolves the installation to diagnose.
///
/// `--root` bypasses discovery's machine scan: the path is taken as given, and it
/// doubles as the DSH home unless `--home` overrides that. Otherwise `--install`
/// selects one discovered installation by id; with neither flag doctor refuses to
/// guess, because diagnosing the wrong installation is worse than not diagnosing
/// at all.
fn select_install(
    install_id: Option<&str>,
    root: Option<&str>,
    home: Option<&str>,
) -> Result<Install, Error> {
    if let Some(root) = root {
        // A typo'd --root must fail instead of diagnosing nothing and exiting 0
        // with "no findings". The path is inspected, never created.
        require_root_dir(root)?;
        let home = home.unwrap_or(root);
        // Profiles are enumerated from the home the same way discovery does, so
        // a tree whose only profile is web, tui, or headless is diagnosed too.
        // Falling back to desktop here would silently report such a tree clean.
        let profiles = o824_discovery::profile_names(Path::new(home));
        let install_id = match install_id {
            Some(id) if !id.is_empty() => id.to_owned(),
            _ => local_install_id(root),
        };
        return Ok(Install {
            install_id,
            kind: o824_discovery::KIND_DESKTOP.to_owned(),
            root: root.to_owned(),
            version: String::new(),
            profiles,
            health: String::new(),
            home: home.to_owned(),
        });
    }

    if install_id.is_some() {
        return Err(Error::new(
            Code::Unsupported,
            "o824 doctor: --install needs the machine scan, which arrives with milestone M3; \
             pass --root <path> for now",
        ));
    }

    Err(Error::new(
        Code::Usage,
        "o824 doctor: pass --install <id> or --root <path>",
    ))
}

/// Checks that `root` names a directory.
fn require_root_dir(root: &str) -> Result<(), Error> {
    let clean = clean_path(root);
    let is_dir = std::fs::metadata(&clean)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    if is_dir {
        return Ok(());
    }
    Err(Error::new(
        Code::InstallDir,
        format!("{clean:?} is not an installation directory"),
    )
    .with_param("root", clean))
}

/// Normalises a path the way the reference implementation's `filepath.Clean`
/// does for the shapes this kernel produces.
fn clean_path(path: &str) -> String {
    o824_model::path::clean(Path::new(path))
        .display()
        .to_string()
}

/// Derives the stable store key for a root the user named directly.
///
/// It is `local-<12 hex of sha256(lowercased realpath)>`: a root names an
/// installation even without an `--install` label, and the plan builder refuses
/// an installation with no id.
pub fn local_install_id(root: &str) -> String {
    use sha2::{Digest, Sha256};

    let cleaned = clean_path(root);
    let resolved = std::fs::canonicalize(&cleaned)
        .map(|path| o824_verify::strip_nt_namespace(&path).display().to_string())
        .unwrap_or(cleaned);
    let mut hasher = Sha256::new();
    hasher.update(resolved.to_lowercase().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("local-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_root_is_e_install_dir() {
        let error = require_root_dir(r"C:\definitely\not\here").expect_err("must fail");
        assert_eq!(error.code, Code::InstallDir);
        assert!(error.fallback.contains("is not an installation directory"));
        assert!(error.params.contains_key("root"));
    }

    #[test]
    fn a_root_without_an_install_flag_still_gets_a_stable_id() {
        let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
        let id = local_install_id(&dir.path().display().to_string());
        assert!(id.starts_with("local-"), "{id}");
        assert_eq!(id.len(), "local-".len() + 12);
        // The same root always produces the same id.
        assert_eq!(id, local_install_id(&dir.path().display().to_string()));
    }

    #[test]
    fn neither_flag_nor_root_is_a_usage_error() {
        let error = select_install(None, None, None).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(error
            .fallback
            .contains("pass --install <id> or --root <path>"));
    }

    #[test]
    fn installing_by_id_is_not_reachable_before_the_machine_scan() {
        let error = select_install(Some("i1"), None, None).expect_err("must fail");
        assert_eq!(error.code, Code::Unsupported);
    }
}
