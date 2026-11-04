//! `o824 update --check`: is there a newer release, and can it be trusted?
//!
//! P4 asks for self-update. This is the half that has to exist **before** anything is ever
//! replaced: read a published manifest, verify it against a key the caller trusts, compare
//! versions, and say what would change. It deliberately has no `--apply`: the design says the
//! build is unsigned, and an unsigned binary that replaces itself is the one thing a launcher
//! must not do without a signed manifest and a decision to trust it.
//!
//! The manifest is read from a file, not fetched. Fetching is a network client, and this
//! workspace has none on purpose — the downloader is the user's browser, and what arrives is
//! then checked here.

use std::io::Write;
use std::path::Path;

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_for, EXIT_FINDINGS, EXIT_OK, EXIT_USAGE};

const USAGE: &str = "\
usage: o824 update --check --manifest <file> [--public-key <hex>|--public-key-file <path>]
                       [--installed <dir>] [--json]

  Reads a published manifest, verifies its signature, and says whether it describes a newer
  version than this one. With --installed, it also says which local files differ from what
  the manifest describes (that is a check of what you have, not of what you would get).

  Nothing is downloaded and nothing is replaced. Exit codes: 0 nothing newer, 1 something
  newer, 2 the arguments or the manifest are wrong, 4 a file could not be read,
  5 the manifest is not signed by the key you trust.
";

/// `o824 update`.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::boolean("check"),
        FlagSpec::valued("manifest"),
        FlagSpec::valued("public-key"),
        FlagSpec::valued("public-key-file"),
        FlagSpec::valued("installed"),
        FlagSpec::valued("version"),
        FlagSpec::boolean("json"),
    ]);
    if let Err(error) = flags.parse("update", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }
    if !flags.flag("check") {
        let error = Error::new(
            Code::Usage,
            "update: this command only checks; pass --check. Replacing files is not implemented",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }
    let Some(manifest_path) = flags.value("manifest") else {
        let error = Error::new(Code::Usage, "update: pass --manifest <file>");
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };

    let manifest = match o824_manifest::load(Path::new(manifest_path)) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    };

    // The trusted key: an argument, a file, or nothing. With nothing, the signature can only
    // be checked against the key the manifest carries — which proves the file is intact and
    // proves nothing about who wrote it. That distinction is printed, not assumed.
    let trusted = flags.value("public-key").map(str::to_owned).or_else(|| {
        flags
            .value("public-key-file")
            .and_then(|path| std::fs::read_to_string(path).ok())
    });
    let trusted = match trusted {
        Some(text) => {
            let key = text
                .trim()
                .trim_start_matches("public key:")
                .trim()
                .to_owned();
            if let Err(error) = o824_manifest::verify_signature(&manifest, Some(&key)) {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
            Some(key)
        }
        None => {
            if let Err(error) = o824_manifest::verify_signature(&manifest, None) {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
            None
        }
    };

    // Compare versions. A manifest whose version cannot be read is not comparable, and
    // saying "unknown" is better than pretending.
    let current = flags
        .value("version")
        .map(str::to_owned)
        .unwrap_or_else(|| o824_version::VERSION.to_owned());
    let current_version = o824_verify::tui::parse_version(&current);
    let published_version = o824_verify::tui::parse_version(&manifest.version);
    let newer = match (&current_version, &published_version) {
        (Some(current), Some(published)) => published > current,
        _ => false,
    };

    let signature_note = match &trusted {
        Some(key) => format!("verified against {key:.16}…"),
        None => "self-signed only: the key came with the manifest".to_owned(),
    };

    let mut differences = Vec::new();
    if let Some(directory) = flags.value("installed") {
        match o824_manifest::verify_files(&manifest, Path::new(directory)) {
            Ok(report) => {
                for path in &report.changed {
                    differences.push(format!("differs:    {path}"));
                }
                for path in &report.missing {
                    differences.push(format!("missing:    {path}"));
                }
                for path in &report.unexpected {
                    differences.push(format!("not listed: {path}"));
                }
            }
            Err(error) => {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
        }
    }

    if flags.flag("json") {
        let payload = serde_json::json!({
            "current": current,
            "published": manifest.version,
            "newer": newer,
            "keyId": manifest.signature.as_ref().map(|signature| signature.key_id.clone()),
            "trusted": trusted.is_some(),
            "files": manifest.files.len(),
            "differences": differences,
        });
        let _ = writeln!(stdout, "{payload}");
    } else {
        let _ = writeln!(stdout, "signature: ok ({signature_note})");
        let _ = writeln!(stdout, "current:   {current}");
        let _ = writeln!(stdout, "published: {}", manifest.version);
        if newer {
            let _ = writeln!(
                stdout,
                "a newer release is published: {} files, nothing downloaded",
                manifest.files.len()
            );
        } else {
            let _ = writeln!(stdout, "nothing newer than what is running");
        }
        for line in &differences {
            let _ = writeln!(stdout, "{line}");
        }
        if !differences.is_empty() {
            let _ = writeln!(
                stdout,
                "the files above are what an update would touch; this command changes nothing"
            );
        }
    }

    if newer {
        EXIT_FINDINGS
    } else {
        EXIT_OK
    }
}

/// Rejects anything that would look like a request to update in place.
///
/// Kept as a function so the rule is visible in one place: there is no code path in this
/// binary that replaces itself.
pub fn refuse_apply(stdout: &mut dyn Write, _stderr: &mut dyn Write) -> i32 {
    let _ = writeln!(
        stdout,
        "update: replacing the running program is not implemented. That needs a signed \
         manifest, a trusted key in the binary, and a decision to trust it — P4, not a flag."
    );
    EXIT_USAGE
}
