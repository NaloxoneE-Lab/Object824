//! The `diagnose` subcommand: write one redacted support bundle.
//!
//! The bundle is allow-listed by construction — the five profile settings files
//! and the newest journals — so credentials and session data have no path into it.

use std::io::Write;
use std::path::PathBuf;

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_of, EXIT_OK};

/// The help text for the diagnose subcommand.
pub const USAGE: &str = "\
Usage:
  o824 diagnose --install <id> --out <path> [--include-paths] [--json]
  o824 diagnose --root <path> [--home <path>] --out <path> [--include-paths] [--json]

Writes one support bundle: a zip containing the build identity, the system
summary, the doctor findings, each profile's settings files, and the newest
install journals.

The bundle is allow-listed, never discovered by walking the installation, so
credentials, session transcripts, and tokens cannot reach it. Every text entry is
redacted by default: the installation root, the DSH home, and %USERPROFILE% become
<ROOT>, <HOME>, and <USERPROFILE>.

Flags:
  --install <id>     diagnose one discovered installation, selected by id
  --root <path>      diagnose this directory instead of scanning the machine
  --home <path>      DSH home whose profiles are collected (--root implies --home)
  --out <path>       write the bundle here; must be a file, not a directory
  --include-paths    keep absolute paths instead of redacting them, for a
                     maintainer who needs the real layout
  --json             print the manifest and nothing else

Exit code 0 means the bundle was written, 2 means the command line was unusable,
4 means a file could not be read, and 8 means the bundle could not be written.
";

/// Runs `o824 diagnose`. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    run_with(args, stdout, stderr, None)
}

/// Runs `o824 diagnose` with an explicit data root, for tests.
pub fn run_with(
    args: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    data_root: Option<&std::path::Path>,
) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("json"),
        FlagSpec::boolean("include-paths"),
        FlagSpec::valued("root"),
        FlagSpec::valued("home"),
        FlagSpec::valued("install"),
        FlagSpec::valued("out"),
    ]);
    if let Err(error) = flags.parse("diagnose", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }
    if let Some(extra) = flags.positional().first() {
        let error = Error::new(
            Code::Usage,
            format!("o824 diagnose: unexpected argument {extra:?}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    }

    let Some(out) = flags.value("out").filter(|out| !out.is_empty()) else {
        let error = Error::new(Code::Usage, "o824 diagnose: pass --out <path>");
        let _ = writeln!(stderr, "{error}");
        return exit_code_of(Some(&error));
    };

    let install = match crate::install::select(
        "diagnose",
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

    let data_root: PathBuf = match data_root {
        Some(root) => root.to_path_buf(),
        None => o824_state::data_root(None),
    };

    let manifest = match o824_diagnose::write(o824_diagnose::Options {
        install,
        data_root,
        out: PathBuf::from(out),
        include_paths: flags.flag("include-paths"),
        now: o824_model::Timestamp::now(),
    }) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_of(Some(&error));
        }
    };

    if flags.flag("json") {
        match serde_json::to_string(&manifest) {
            Ok(text) => {
                let _ = writeln!(stdout, "{text}");
            }
            Err(error) => {
                let error = Error::new(
                    Code::Internal,
                    format!("o824 diagnose: cannot encode the manifest: {error}"),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_of(Some(&error));
            }
        }
        return EXIT_OK;
    }

    let _ = writeln!(
        stdout,
        "wrote {} ({} entries{})",
        out,
        manifest.entries.len(),
        if manifest.redacted { ", redacted" } else { "" }
    );
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn run_capture(args: &[&str], data_root: &std::path::Path) -> (i32, String, String) {
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
    fn a_bundle_is_written_and_reported() {
        let fixture = o824_fixtures::build("git-dep").expect("fixture");
        let store = TempDir::new("o824-diagnose-cli-").expect("store");
        let root = fixture.root().display().to_string();
        let out = store.path().join("bundle.zip");
        let out_text = out.display().to_string();

        let (code, stdout, stderr) =
            run_capture(&["--root", &root, "--out", &out_text], store.path());
        assert_eq!(code, 0, "{stderr}");
        assert!(stdout.contains("redacted"), "{stdout}");
        assert!(out.is_file());
    }

    #[test]
    fn json_prints_the_manifest() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = TempDir::new("o824-diagnose-cli-").expect("store");
        let root = fixture.root().display().to_string();
        let out = store.path().join("bundle.zip");
        let out_text = out.display().to_string();

        let (code, stdout, stderr) = run_capture(
            &["--root", &root, "--out", &out_text, "--json"],
            store.path(),
        );
        assert_eq!(code, 0, "{stderr}");
        let manifest: serde_json::Value = serde_json::from_str(&stdout).expect("json");
        assert_eq!(manifest["redacted"], true);
        assert_eq!(manifest["install"]["root"], "<ROOT>");
    }

    #[test]
    fn a_missing_out_is_a_usage_error() {
        let store = TempDir::new("o824-diagnose-cli-").expect("store");
        let root = store.path().display().to_string();
        let (code, _, stderr) = run_capture(&["--root", &root], store.path());
        assert_eq!(code, crate::EXIT_USAGE);
        assert!(stderr.contains("--out"), "{stderr}");
    }
}
