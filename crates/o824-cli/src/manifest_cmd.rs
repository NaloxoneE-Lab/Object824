//! `o824 manifest`, `o824 manifest-keygen`, and `o824 verify-manifest`.
//!
//! These are the three commands that make an unsigned release checkable: publish a list of
//! files with their hashes, sign the list, and let anyone verify both the list and the files
//! against the key the publisher announces.

use std::io::Write;
use std::path::{Path, PathBuf};

use o824_errs::Error;

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_for, EXIT_OK};

const USAGE: &str = "\
usage: o824 manifest --root <dir> --out <file> [--version <v>] [--key <hex>|--key-file <path>]
       o824 manifest-keygen [--out <path>]
       o824 verify-manifest --manifest <file> [--root <dir>] [--public-key <hex>]

  manifest         list every file under --root with its sha256, optionally sign it
  manifest-keygen  make an ed25519 key pair; the secret is written, the public key printed
                   with --from <secret>, print the public key of an existing secret instead
  verify-manifest  check the signature, then the files against it

Exit codes: 0 clean, 2 the arguments or the manifest are wrong, 5 the release does not match.
";

/// `o824 manifest`.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::valued("root"),
        FlagSpec::valued("out"),
        FlagSpec::valued("version"),
        FlagSpec::valued("key"),
        FlagSpec::valued("key-file"),
    ]);
    if let Err(error) = flags.parse("manifest", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let Some(root) = flags.value("root") else {
        let error = Error::new(o824_errs::Code::Usage, "manifest: pass --root <dir>");
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };
    let Some(out) = flags.value("out") else {
        let error = Error::new(o824_errs::Code::Usage, "manifest: pass --out <file>");
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };
    let version = flags
        .value("version")
        .map(str::to_owned)
        .unwrap_or_else(|| o824_version::VERSION.to_owned());
    let root = PathBuf::from(root);

    let mut manifest =
        match o824_manifest::build(&root, &version, &o824_model::Timestamp::now().to_rfc3339()) {
            Ok(manifest) => manifest,
            Err(error) => {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
        };

    let secret = match flags.value("key").map(str::to_owned).or_else(|| {
        flags
            .value("key-file")
            .and_then(|path| std::fs::read_to_string(path).ok())
    }) {
        Some(text) => match o824_manifest::parse_key(&text) {
            Ok(key) => Some(key),
            Err(error) => {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
        },
        None => None,
    };

    if let Some(key) = &secret {
        if let Err(error) = o824_manifest::sign(&mut manifest, key) {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    }

    if let Err(error) = o824_manifest::save(&manifest, Path::new(out)) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let signed = if manifest.signature.is_some() {
        "signed"
    } else {
        "UNSIGNED"
    };
    let _ = writeln!(
        stdout,
        "manifest: {} files, {signed}, written to {out}",
        manifest.files.len()
    );
    if manifest.signature.is_none() {
        let _ = writeln!(
            stdout,
            "  it proves nothing yet: sign it with --key or --key-file"
        );
    }
    EXIT_OK
}

/// `o824 manifest-keygen`.
pub fn keygen(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![FlagSpec::valued("out"), FlagSpec::valued("from")]);
    if let Err(error) = flags.parse("manifest-keygen", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    // `--from` derives the public key from an existing secret instead of making a new pair:
    // a release script has the secret and needs to know which public key to publish.
    if let Some(source) = flags.value("from") {
        let text = match std::fs::read_to_string(source) {
            Ok(text) => text,
            Err(_) => source.to_owned(),
        };
        let key = match o824_manifest::parse_key(&text) {
            Ok(key) => key,
            Err(error) => {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
        };
        let _ = writeln!(stdout, "public key: {}", o824_manifest::public_hex(&key));
        return EXIT_OK;
    }

    let key = o824_manifest::new_key();
    let public = o824_manifest::public_hex(&key);
    let secret = key.to_bytes();
    let secret_hex: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();

    match flags.value("out") {
        Some(path) => {
            if let Err(error) = std::fs::write(path, format!("{secret_hex}\n")) {
                let error = Error::new(
                    o824_errs::Code::Permission,
                    format!("manifest-keygen: cannot write {path:?}: {error}"),
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
            let _ = writeln!(stdout, "secret key written to {path}");
        }
        None => {
            let _ = writeln!(stdout, "secret key: {secret_hex}");
        }
    }
    let _ = writeln!(stdout, "public key: {public}");
    let _ = writeln!(
        stdout,
        "  publish the public key; keep the secret one off this machine's releases"
    );
    EXIT_OK
}

/// `o824 verify-manifest`.
pub fn verify(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::valued("manifest"),
        FlagSpec::valued("root"),
        FlagSpec::valued("public-key"),
    ]);
    if let Err(error) = flags.parse("verify-manifest", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let Some(manifest_path) = flags.value("manifest") else {
        let error = Error::new(
            o824_errs::Code::Usage,
            "verify-manifest: pass --manifest <file>",
        );
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

    // The signature first: a file comparison against a manifest nobody signed proves the
    // files are self-consistent, not that they are the published ones.
    if let Err(error) = o824_manifest::verify_signature(&manifest, flags.value("public-key")) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }
    let key = manifest
        .signature
        .as_ref()
        .map(|signature| signature.key_id.clone())
        .unwrap_or_default();
    let _ = writeln!(stdout, "signature: ok ({key})");

    let Some(root) = flags.value("root") else {
        let _ = writeln!(
            stdout,
            "files: not checked (pass --root <dir> to compare them)"
        );
        return EXIT_OK;
    };

    let report = match o824_manifest::verify_files(&manifest, Path::new(root)) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    };
    for path in &report.changed {
        let _ = writeln!(stdout, "changed:    {path}");
    }
    for path in &report.missing {
        let _ = writeln!(stdout, "missing:    {path}");
    }
    for path in &report.unexpected {
        let _ = writeln!(stdout, "unexpected: {path}");
    }
    if report.is_clean() {
        let _ = writeln!(stdout, "files: ok ({} checked)", report.checked);
        EXIT_OK
    } else {
        let _ = writeln!(
            stdout,
            "files: {} checked, {} changed, {} missing, {} unexpected",
            report.checked,
            report.changed.len(),
            report.missing.len(),
            report.unexpected.len()
        );
        crate::EXIT_STATE_DRIFT
    }
}
