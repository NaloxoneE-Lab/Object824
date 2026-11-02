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
    let mut envelope = o824_verify::doctor(&install, &data_root);

    // F11 is a statement about the machine, not about this installation: two forms
    // sharing one DSH home at different versions. It is appended here because the
    // per-installation envelope has nowhere else to learn about the other forms.
    //
    // `--root` means "diagnose this tree in isolation" — the lab and the golden
    // fixtures rely on it — so the machine is only consulted when the installation
    // came from the machine.
    if flags.value("root").is_none() {
        let machine = o824_discovery::discover(&o824_discovery::Options {
            home: Some(install.home.clone()),
            ..Default::default()
        })
        .unwrap_or_default();
        if let Some(drift) = o824_verify::version_drift(&machine) {
            envelope.findings.push(drift);
        }
    }

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
/// The rules are shared with `discover` and `repair`: an explicit `--root` is
/// inspected, and a named `--install` goes through the machine scan.
fn select_install(
    install_id: Option<&str>,
    root: Option<&str>,
    home: Option<&str>,
) -> Result<Install, Error> {
    crate::install::select("doctor", install_id, root, home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neither_flag_nor_root_is_a_usage_error() {
        let error = select_install(None, None, None).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(error
            .fallback
            .contains("pass --install <id> or --root <path>"));
    }

    /// Selecting by id scans the machine, so an id that names nothing is a usage
    /// error rather than a silent empty diagnosis. The machine in the test may
    /// have no installation at all, which also yields the usage error.
    #[test]
    fn an_unknown_install_id_is_a_usage_error() {
        let error =
            select_install(Some("i-definitely-not-there"), None, None).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(
            error.fallback.contains("no installation with id"),
            "{error}"
        );
    }

    /// With an explicit root the id is a label, not a selector, so it never
    /// reaches the machine scan.
    #[test]
    fn an_explicit_root_and_an_id_combine() {
        let directory = o824_fixtures::TempDir::new("o824-doctor-").expect("temp dir");
        let root = directory.path().display().to_string();
        let install = select_install(Some("labelled"), Some(&root), None).expect("select");
        assert_eq!(install.install_id, "labelled");
        assert_eq!(install.root, root);
    }
}
