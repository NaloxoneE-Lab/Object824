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
use crate::{exit_code_for, EXIT_FINDINGS, EXIT_OK, EXIT_STATE_DRIFT, EXIT_USAGE};

const USAGE: &str = "\
usage: o824 update --check --manifest <file> [--public-key <hex>|--public-key-file <path>]
                       [--installed <dir>] [--json]
       o824 update --fetch --manifest-url <url> --base-url <url> --into <dir>
                       [--public-key <hex>]

  --check reads a manifest you already have: it verifies the signature, compares versions, and
  with --installed says which local files differ.

  --fetch downloads a manifest, verifies its signature before anything else arrives, then
  downloads every file it lists and checks each hash. A file path that would leave the target
  directory is refused rather than followed.

  Neither mode replaces anything. Exit codes: 0 nothing newer, 1 something newer, 2 the
  arguments or the manifest are wrong, 4 a file could not be read, 5 a signature or a hash did
  not match, 6 a download failed.

  Run this from an installed location: a program started from a sandboxed build folder runs at
  low integrity, and the system downloader refuses requests from it.
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
        FlagSpec::boolean("fetch"),
        FlagSpec::valued("manifest-url"),
        FlagSpec::valued("base-url"),
        FlagSpec::valued("into"),
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
    // `--fetch` asks the same trust question about bytes that arrive over the network:
    // download, verify, stop. Replacing is still not a flag.
    if flags.flag("fetch") {
        return fetch(&flags, stdout, stderr);
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

    // Who do we trust? A key the caller names, else the key compiled into this build, else
    // nobody — in which case the signature is only checked against the key the manifest
    // carries, which proves the file is intact and proves nothing about who wrote it.
    let from_argument = flags
        .value("public-key")
        .map(str::to_owned)
        .or_else(|| {
            flags
                .value("public-key-file")
                .and_then(|path| std::fs::read_to_string(path).ok())
        })
        .map(|text| {
            text.trim()
                .trim_start_matches("public key:")
                .trim()
                .to_owned()
        });
    let trust = resolve_trust(from_argument, o824_manifest::built_in_key());

    let trusted = match &trust {
        Trust::Named(key) | Trust::BuiltIn(key) => {
            if let Err(error) = o824_manifest::verify_signature(&manifest, Some(key)) {
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
            Some(key.clone())
        }
        Trust::Nobody => {
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

    let signature_note = match (&trust, &trusted) {
        (Trust::BuiltIn(key), _) => format!(
            "verified against the key built into this binary ({}…)",
            &key[..16.min(key.len())]
        ),
        (Trust::Named(key), _) => format!("verified against {key:.16}…"),
        (Trust::Nobody, _) => "self-signed only: the key came with the manifest".to_owned(),
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
            "trust": match &trust {
                Trust::BuiltIn(_) => "built-in",
                Trust::Named(_) => "argument",
                Trust::Nobody => "self-signed",
            },
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

/// Downloads a release into a directory and verifies every byte of it.
///
/// The order matters: the manifest is fetched and **verified before** anything else arrives, so
/// a hostile mirror cannot hand out an unsigned list and have files follow it. Every file must
/// then match the hash and size the manifest recorded, and every path must stay inside the
/// target directory — a manifest that names `..` or an absolute path is refused, not followed.
fn fetch(flags: &FlagSet, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let (Some(manifest_url), Some(base_url), Some(into)) = (
        flags.value("manifest-url"),
        flags.value("base-url"),
        flags.value("into"),
    ) else {
        let error = Error::new(
            Code::Usage,
            "update --fetch: pass --manifest-url <url> --base-url <url> --into <dir>",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };
    let directory = Path::new(into);
    if let Err(error) = std::fs::create_dir_all(directory) {
        let error = Error::new(
            Code::Permission,
            format!("update: cannot create {into:?}: {error}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let manifest_path = directory.join(o824_manifest::MANIFEST_FILE);
    let _ = writeln!(stdout, "fetching {manifest_url}");
    if let Err(error) = o824_install::download(manifest_url, &manifest_path) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let manifest = match o824_manifest::load(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    };
    let trusted = flags.value("public-key").map(|text| text.trim().to_owned());
    if let Err(error) = o824_manifest::verify_signature(&manifest, trusted.as_deref()) {
        let _ = writeln!(stderr, "{error}");
        let _ = writeln!(
            stderr,
            "update: nothing else was downloaded; the manifest is not signed by a key you trust"
        );
        return exit_code_for(error.code);
    }
    let key_id = manifest
        .signature
        .as_ref()
        .map(|signature| signature.key_id.clone())
        .unwrap_or_default();
    let _ = writeln!(
        stdout,
        "manifest verified ({key_id}), {} files",
        manifest.files.len()
    );

    for entry in &manifest.files {
        let Some(relative) = safe_relative(&entry.path) else {
            let _ = writeln!(
                stderr,
                "update: the manifest asks for {:?}, which is outside the target directory",
                entry.path
            );
            return EXIT_STATE_DRIFT;
        };
        let url = format!("{}/{}", base_url.trim_end_matches('/'), entry.path);
        let destination = directory.join(&relative);
        let _ = writeln!(stdout, "  {}", entry.path);
        if let Err(error) = o824_install::download(&url, &destination) {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    }

    let report = match o824_manifest::verify_files(&manifest, directory) {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    };
    for path in &report.changed {
        let _ = writeln!(stderr, "changed after download: {path}");
    }
    for path in &report.missing {
        let _ = writeln!(stderr, "missing after download: {path}");
    }
    if !report.is_clean() {
        let _ = writeln!(
            stderr,
            "update: the downloaded files do not match the manifest"
        );
        return EXIT_STATE_DRIFT;
    }

    let _ = writeln!(
        stdout,
        "staged in {into}: {} files, every hash matches. Nothing was replaced.",
        report.checked
    );
    let _ = writeln!(
        stdout,
        "to use it, quit O824 and copy these files over the installed ones; doing that \
         automatically needs a decision about trusting the key first."
    );
    EXIT_OK
}

/// A manifest path that stays inside the target directory.
///
/// The manifest arrives from the network. `..`, an absolute path, or a drive prefix would let it
/// write anywhere the user can write, which is the difference between a downloader and a
/// delivery mechanism for someone else's files.
pub fn safe_relative(path: &str) -> Option<std::path::PathBuf> {
    let path = path.replace('\\', "/");
    let path = path.trim_start_matches("./");
    if path.is_empty() || path.starts_with('/') || path.contains(':') {
        return None;
    }
    let mut relative = std::path::PathBuf::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            other => relative.push(other),
        }
    }
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

/// Where the trust for a check comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// A key the caller named with `--public-key` or `--public-key-file`.
    Named(String),
    /// The key compiled into this build.
    BuiltIn(String),
    /// No key at all: only the manifest's own key is available.
    Nobody,
}

/// Decides which key to trust.
///
/// An explicit key wins over the built-in one, because someone checking a build other than
/// this one should not have to rebuild to say so. The built-in key is the fallback that makes
/// `o824 update --check` meaningful for an ordinary user: no argument, still real trust.
pub fn resolve_trust(from_argument: Option<String>, built_in: Option<&str>) -> Trust {
    match (from_argument, built_in) {
        (Some(key), _) if !key.is_empty() => Trust::Named(key),
        (_, Some(key)) => Trust::BuiltIn(key.to_owned()),
        _ => Trust::Nobody,
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
