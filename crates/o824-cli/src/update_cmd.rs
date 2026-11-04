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
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{
    exit_code_for, EXIT_CONFIRMATION_REQUIRED, EXIT_FINDINGS, EXIT_OK, EXIT_STATE_DRIFT, EXIT_USAGE,
};

const USAGE: &str = "\
usage: o824 update --check --manifest <file> [--public-key <hex>|--public-key-file <path>]
                       [--installed <dir>] [--json]
       o824 update --fetch --manifest-url <url> --base-url <url> --into <dir>
                       [--public-key <hex>]
       o824 update --apply --staged <dir> [--target <dir>] --yes [--public-key <hex>] [--force]

  --check reads a manifest you already have: verifies the signature, compares versions, and
  with --installed says which local files differ.

  --fetch downloads a manifest, verifies its signature before anything else arrives, then
  downloads every file it lists and checks each hash. A file path that would leave the target
  directory is refused rather than followed.

  --apply replaces the installed files with a staged folder that verifies against its signed
  manifest. It needs --yes, keeps every replaced file as <name>.bak, and puts them all back if
  any step fails. A running program cannot be overwritten on Windows, so --apply starts a copy
  of itself in %TEMP% and lets that copy do the swap after this one exits; the window restarts
  when it is done.

  Exit codes: 0 done, 1 something newer, 2 the arguments or the manifest are wrong, 3 --yes was
  missing, 4 a file could not be read or written, 5 a signature or a hash did not match,
  6 a download failed.

  Run these from an installed location: a program started from a sandboxed build folder runs at
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
        FlagSpec::boolean("apply"),
        FlagSpec::boolean("swap"),
        FlagSpec::valued("staged"),
        FlagSpec::valued("target"),
        FlagSpec::boolean("force"),
        FlagSpec::boolean("yes"),
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
    // `--apply` is the only mode that changes anything, and it says so first.
    if flags.flag("apply") {
        return apply_update(&flags, stdout, stderr);
    }
    if flags.flag("swap") {
        return swap(&flags, stdout, stderr);
    }
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

/// Replaces the installed files with a **verified** staged release, then restarts the window.
///
/// Three rules, in this order:
///  * nothing is replaced unless the staged folder verifies against the signed manifest — the
///    check happens here, not in the caller;
///  * nothing is replaced unless the caller said `--yes` (§20.7), because this deletes the
///    program someone is using — a `.bak` copy of each replaced file is kept beside it;
///  * nothing is replaced by *this* process: a running executable cannot be overwritten on
///    Windows, so this copies itself into a temporary folder and lets that copy do the swap
///    once this one has exited.
fn apply_update(flags: &FlagSet, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let Some(staged) = flags.value("staged") else {
        let error = Error::new(Code::Usage, "update --apply: pass --staged <dir>");
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };
    let target = match flags.value("target") {
        Some(path) => PathBuf::from(path),
        None => o824_install::default_directory(),
    };
    if !flags.flag("yes") {
        let error = Error::new(
            Code::ConfirmationRequired,
            "update --apply replaces the installed program; pass --yes to confirm",
        );
        let _ = writeln!(stderr, "{error}");
        return EXIT_CONFIRMATION_REQUIRED;
    }

    // Verify before touching anything, every time: a folder that passed a check in another
    // process is not evidence about the bytes on disk now.
    let manifest_path = Path::new(staged).join(o824_manifest::MANIFEST_FILE);
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
        return exit_code_for(error.code);
    }
    match o824_manifest::verify_files(&manifest, Path::new(staged)) {
        Ok(report) if report.is_clean() => {}
        Ok(report) => {
            for path in report.changed.iter().chain(report.missing.iter()) {
                let _ = writeln!(stderr, "does not match the manifest: {path}");
            }
            let _ = writeln!(
                stderr,
                "update: the staged folder is not what was signed; refusing"
            );
            return EXIT_STATE_DRIFT;
        }
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    }

    // A downgrade only when it is asked for: the usual reason to be here is an update, and
    // silently installing an older build is a surprise nobody wants.
    let current = o824_verify::tui::parse_version(o824_version::VERSION);
    let published = o824_verify::tui::parse_version(&manifest.version);
    if let (Some(current), Some(published)) = (&current, &published) {
        if published < current && !flags.flag("force") {
            let error = Error::new(
                Code::Usage,
                format!(
                    "update: {:?} is older than {}. Pass --force to install it anyway",
                    manifest.version,
                    o824_version::VERSION
                ),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    }

    let Ok(exe) = std::env::current_exe() else {
        let error = Error::new(
            Code::Internal,
            "update: cannot tell where this program lives",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };

    // A fresh folder per run, so two attempts cannot fight over the same binary.
    let work = std::env::temp_dir().join(format!("o824-update-{}", std::process::id()));
    if let Err(error) = std::fs::create_dir_all(&work) {
        let error = Error::new(
            Code::Permission,
            format!(
                "update: cannot create {:?}: {error}",
                work.display().to_string()
            ),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }
    let helper = work.join("o824.exe");
    if let Err(error) = std::fs::copy(&exe, &helper) {
        let error = Error::new(
            Code::Permission,
            format!("update: cannot prepare the swap: {error}"),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let spawn = std::process::Command::new(&helper)
        .arg("update")
        .arg("--swap")
        .arg("--staged")
        .arg(staged)
        .arg("--target")
        .arg(target.as_os_str())
        .arg("--yes")
        .spawn();
    match spawn {
        Ok(_) => {
            let _ = writeln!(
                stdout,
                "verified {} files; the swap runs once this process exits and the window restarts",
                manifest.files.len()
            );
            let _ = writeln!(
                stdout,
                "the files it replaces are kept beside them as .bak until the next update"
            );
            EXIT_OK
        }
        Err(error) => {
            let error = Error::new(
                Code::Internal,
                format!("update: cannot start the swap: {error}"),
            );
            let _ = writeln!(stderr, "{error}");
            exit_code_for(error.code)
        }
    }
}

/// The swap itself, run from the temporary copy once the launcher has exited.
///
/// Every replaced file is moved to `<name>.bak` first, and any failure puts all of them back:
/// an update that half-applies is worse than one that refuses to.
fn swap(flags: &FlagSet, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let (Some(staged), Some(target)) = (flags.value("staged"), flags.value("target")) else {
        let error = Error::new(
            Code::Usage,
            "update --swap: pass --staged <dir> --target <dir>",
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    };
    let staged = Path::new(staged);
    let target = Path::new(target);

    // Give the process that started us time to exit and release its executable.
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let manifest = match o824_manifest::load(&staged.join(o824_manifest::MANIFEST_FILE)) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    };

    let plan = swap_plan(&manifest, staged, target);
    let mut done: Vec<SwapStep> = Vec::new();
    for step in &plan {
        if let Err(error) = step.execute() {
            let _ = writeln!(stderr, "update: {error}");
            for finished in done.iter().rev() {
                let _ = finished.undo();
            }
            let _ = writeln!(stderr, "update: every replaced file was put back");
            return exit_code_for(Code::Permission);
        }
        done.push(step.clone());
    }

    let _ = writeln!(
        stdout,
        "replaced {} files in {}",
        plan.len(),
        target.display()
    );
    let _ = writeln!(stdout, "the previous ones are beside them as .bak");

    // Start the new window.
    let gui = target.join(o824_install::FILES[1]);
    if gui.is_file() {
        let _ = std::process::Command::new(&gui).spawn();
    }
    EXIT_OK
}

/// One file replacement: move the old file aside, put the new one in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapStep {
    /// The verified new file.
    pub from: PathBuf,
    /// Where it has to end up.
    pub to: PathBuf,
    /// Where the old file is kept.
    pub backup: PathBuf,
}

impl SwapStep {
    fn execute(&self) -> Result<(), String> {
        if self.backup.exists() {
            std::fs::remove_file(&self.backup).map_err(|error| {
                format!(
                    "cannot clear {:?}: {error}",
                    self.backup.display().to_string()
                )
            })?;
        }
        if self.to.exists() {
            std::fs::rename(&self.to, &self.backup).map_err(|error| {
                format!(
                    "cannot move {:?} aside: {error}",
                    self.to.display().to_string()
                )
            })?;
        }
        std::fs::copy(&self.from, &self.to).map_err(|error| {
            // Put the old file back before reporting: a missing program is worse than an old one.
            if self.backup.exists() {
                let _ = std::fs::rename(&self.backup, &self.to);
            }
            format!(
                "cannot put {:?} in place: {error}",
                self.to.display().to_string()
            )
        })?;
        Ok(())
    }

    fn undo(&self) -> Result<(), String> {
        let _ = std::fs::remove_file(&self.to);
        if self.backup.exists() {
            std::fs::rename(&self.backup, &self.to).map_err(|error| {
                format!(
                    "cannot restore {:?}: {error}",
                    self.to.display().to_string()
                )
            })?;
        }
        Ok(())
    }
}

/// What a swap would do, without doing it.
///
/// Separated so the decision — which files, which backup names, and that a path cannot escape
/// the installation — can be tested without touching a real one.
pub fn swap_plan(
    manifest: &o824_manifest::Manifest,
    staged: &Path,
    target: &Path,
) -> Vec<SwapStep> {
    manifest
        .files
        .iter()
        .filter_map(|entry| {
            let relative = safe_relative(&entry.path)?;
            let from = staged.join(&relative);
            let to = target.join(&relative);
            let mut backup = to.clone();
            let name = to.file_name()?.to_string_lossy().to_string();
            backup.set_file_name(format!("{name}.bak"));
            Some(SwapStep { from, to, backup })
        })
        .collect()
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
