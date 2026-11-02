//! The settings snapshots that back every repair: `apply`'s first step is always
//! a snapshot that reached disk and verified.
//!
//! A snapshot is a directory:
//!
//! ```text
//! <dataRoot>\snapshots\<installID>\<ID>\
//!     files\<rel>   an exact copy of every captured file
//!     meta.json     the marshalled Snapshot, written last
//! ```
//!
//! The layout keeps the installation's relative paths, so restoring is a copy
//! back to `<scope root>\<rel>` with no translation table. The scope root is
//! recorded in `meta.json`: it is the DSH home in the real layout, which is not
//! the application root, and a restore must never write `profiles\` into the
//! application directory.
//!
//! Only the files a repair can touch are captured (spec §16.1): each profile's
//! `package.json`, `cordis.patch.yml`, `cordis.yml`, `pnpm-lock.yaml` and
//! `pnpm-workspace.yaml`, plus the install manifest (`version`). `node_modules`
//! is never captured — it is a generated tree, not settings.
//!
//! `meta.json` is the completeness marker. It is written only after every copy
//! has been compared against its source, so a crash mid-snapshot leaves a
//! directory without `meta.json`, and [`list`] ignores such a directory rather
//! than reporting a snapshot that cannot be restored.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use o824_errs::{Code, Error};
use o824_model::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The directory under the data root that holds every installation's snapshots.
const SNAPSHOTS_DIR_NAME: &str = "snapshots";
/// The subdirectory that mirrors the captured files.
const FILES_DIR_NAME: &str = "files";
/// Written last; marks the snapshot complete.
const META_FILE_NAME: &str = "meta.json";
/// How many complete snapshots per installation are kept.
///
/// It is a constant, not a flag: retention is a guarantee of the store, not a
/// per-run choice.
pub const RETENTION: usize = 5;
/// The number of random hex characters appended to the timestamp prefix, so
/// quickly repeated creates cannot collide.
const RANDOM_HEX_LEN: usize = 4;
/// Bounds the collision retry loop.
const MAX_CREATE_ATTEMPTS: usize = 8;

/// The install manifest at the install root.
const VERSION_FILE_NAME: &str = "version";
/// The pnpm build whitelist at the install root.
const WORKSPACE_FILE_NAME: &str = "pnpm-workspace.yaml";
/// The DSH home directory holding one directory per profile.
const PROFILES_DIR_NAME: &str = "profiles";

/// The per-profile files §16.1 lets a repair touch.
const PROFILE_FILE_NAMES: [&str; 5] = [
    "package.json",
    "cordis.patch.yml",
    "cordis.yml",
    "pnpm-lock.yaml",
    WORKSPACE_FILE_NAME,
];

/// The install-root files a repair can rewrite.
const INSTALL_FILE_NAMES: [&str; 1] = [VERSION_FILE_NAME];

/// One captured file: its slash-separated path relative to the scope root, the
/// sha256 of its bytes, and its size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Root-relative, slash-separated.
    pub rel: String,
    /// Lowercase hex sha256 of the captured bytes.
    pub sha256: String,
    /// The file's size in bytes.
    pub size: i64,
}

/// One captured installation state.
///
/// `dir` is the absolute directory that holds `files\` and `meta.json`. `root` is
/// the scope root the files were captured from — the DSH home in the real layout,
/// where the root and the home differ — so a restore can never silently write
/// `profiles\` into the application directory. `root` is omitted for documents
/// written before it existed; the restore then falls back to its caller's root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// The snapshot id: the directory name.
    pub id: String,
    /// The installation the snapshot belongs to.
    pub install_id: String,
    /// When the snapshot was taken.
    pub created_at: Timestamp,
    /// The install version, when it could be read.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub install_version: String,
    /// The scope root the files were captured from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root: String,
    /// The absolute directory holding the snapshot.
    pub dir: String,
    /// The captured files.
    #[serde(default)]
    pub files: Vec<FileEntry>,
}

/// Captures the settings files of the installation at `root` into a new snapshot
/// under `data_root`, verifies every copy against its source, writes `meta.json`,
/// then prunes older snapshots down to the newest [`RETENTION`].
///
/// The install version is recorded best-effort: discovery is allowed to reject a
/// damaged tree, because a damaged tree is exactly what a repair snapshots.
///
/// A prune failure returns the complete, valid snapshot together with the error:
/// the snapshot itself is usable, and only the housekeeping failed.
pub fn create(data_root: &Path, install_id: &str, root: &Path) -> Result<Snapshot, Error> {
    validate_install_id(install_id)?;
    let clean_root = validate_scope_root(root)?;

    let install_dir = data_root.join(SNAPSHOTS_DIR_NAME).join(install_id);
    fs::create_dir_all(&install_dir)
        .map_err(|error| file_error("create the snapshot store", &install_dir, &error))?;

    let now = Timestamp::now();
    let dir = make_snapshot_dir(&install_dir, now)?;

    let sources = match collect(&clean_root) {
        Ok(sources) => sources,
        Err(error) => {
            let _ = fs::remove_dir_all(&dir);
            return Err(error);
        }
    };

    let mut snapshot = Snapshot {
        id: directory_name(&dir),
        install_id: install_id.to_owned(),
        created_at: now,
        install_version: String::new(),
        root: clean_root.display().to_string(),
        dir: dir.display().to_string(),
        files: Vec::with_capacity(sources.len()),
    };
    // A damaged tree still gets a snapshot, so the version is read best-effort and
    // straight from the tree: validating the whole installation here would pull in
    // the ambient DSH home for a field that does not need it.
    snapshot.install_version = o824_discovery::read_version(&clean_root, None);

    for source in &sources {
        match copy_into(&dir, source) {
            Ok(entry) => snapshot.files.push(entry),
            Err(error) => {
                // The directory has no meta.json, so `list` already ignores it;
                // drop it only to keep a failed run from accumulating junk.
                let _ = fs::remove_dir_all(&dir);
                return Err(error);
            }
        }
    }

    // meta.json is written last: its presence is what makes the snapshot
    // restorable.
    if let Err(error) = write_meta(&dir, &snapshot) {
        let _ = fs::remove_dir_all(&dir);
        return Err(error);
    }

    if let Err(error) = prune(&install_dir, &snapshot.id) {
        return Ok(snapshot).and(Err(error));
    }
    Ok(snapshot)
}

/// [`create`] for callers that want a checkpoint only when the settings actually
/// changed.
///
/// It enumerates and hashes exactly the scope `create` captures and compares that
/// `rel -> sha256` set with the newest complete snapshot. When the two are
/// identical it returns that snapshot with `created == false` and writes nothing.
pub fn create_if_changed(
    data_root: &Path,
    install_id: &str,
    root: &Path,
) -> Result<(Snapshot, bool), Error> {
    validate_install_id(install_id)?;
    let clean_root = validate_scope_root(root)?;

    let snapshots = list(data_root, install_id)?;
    if let Some(newest) = snapshots.first() {
        let current = hash_scope(&clean_root)?;
        if same_scope(newest, &current) {
            return Ok((newest.clone(), false));
        }
    }

    let snapshot = create(data_root, install_id, &clean_root)?;
    Ok((snapshot, true))
}

/// The complete snapshots of `install_id`, newest first.
///
/// A missing store is not an error: an installation that has never been repaired
/// simply has no snapshots. Directories without a readable `meta.json` are
/// skipped, because a snapshot that never finished writing is not a snapshot.
pub fn list(data_root: &Path, install_id: &str) -> Result<Vec<Snapshot>, Error> {
    validate_install_id(install_id)?;
    list_dir(&data_root.join(SNAPSHOTS_DIR_NAME).join(install_id))
}

/// Copies every captured file back into the snapshot's scope root, creating
/// parent directories, and proves each restored file against the snapshot.
///
/// The destination is the snapshot's own root when the document names one: a
/// snapshot taken over the DSH home belongs in the DSH home, even when the caller
/// passed a different installation root. A non-empty root that disagrees is
/// `E_STATE_DRIFT` rather than a guess, because restoring `profiles\` into the
/// application directory is silent data damage.
///
/// Any other failure — a snapshot file that no longer matches its recorded hash,
/// a path that would escape the scope root, an I/O error — is also
/// `E_STATE_DRIFT`: a restore that cannot prove it worked must not report
/// success.
pub fn restore(data_root: &Path, snapshot: &Snapshot, root: &Path) -> Result<(), Error> {
    let clean_root = restore_root(snapshot, root)?;
    let dir = if snapshot.dir.is_empty() {
        // A hand-built document may not know its directory; the ruled layout
        // derives it. Both components are user-supplied, so each must be exactly
        // one path element before it is joined.
        validate_install_id(&snapshot.install_id)?;
        validate_snapshot_id(&snapshot.id)?;
        data_root
            .join(SNAPSHOTS_DIR_NAME)
            .join(&snapshot.install_id)
            .join(&snapshot.id)
    } else {
        PathBuf::from(&snapshot.dir)
    };
    let files_dir = dir.join(FILES_DIR_NAME);

    for entry in &snapshot.files {
        let source = join_under(&files_dir, &entry.rel)
            .map_err(|cause| drift_error(snapshot, entry, "snapshot file path", &cause))?;
        let destination = join_under(&clean_root, &entry.rel)
            .map_err(|cause| drift_error(snapshot, entry, "install path", &cause))?;

        // Prove the snapshot copy is still the bytes that were captured before
        // writing them over the installation.
        let (source_hash, source_size) = hash_file(&source).map_err(|cause| {
            drift_error(snapshot, entry, "read snapshot file", &cause.to_string())
        })?;
        if source_hash != entry.sha256 || source_size != entry.size {
            return Err(drift_error(
                snapshot,
                entry,
                "snapshot file content",
                &format!(
                    "sha256 {source_hash} ({source_size} bytes), want {} ({} bytes)",
                    entry.sha256, entry.size
                ),
            ));
        }

        copy_file(&source, &destination).map_err(|cause| {
            drift_error(snapshot, entry, "write install file", &cause.to_string())
        })?;
        let (written_hash, written_size) = hash_file(&destination).map_err(|cause| {
            drift_error(snapshot, entry, "read restored file", &cause.to_string())
        })?;
        if written_hash != entry.sha256 || written_size != entry.size {
            return Err(drift_error(
                snapshot,
                entry,
                "restored file content",
                &format!(
                    "sha256 {written_hash} ({written_size} bytes), want {} ({} bytes)",
                    entry.sha256, entry.size
                ),
            ));
        }
    }
    Ok(())
}

/// Decides which directory a restore writes into.
fn restore_root(snapshot: &Snapshot, caller_root: &Path) -> Result<PathBuf, Error> {
    let snapshot_root = snapshot.root.trim();
    let caller = caller_root.display().to_string();
    let caller = caller.trim();
    match (snapshot_root.is_empty(), caller.is_empty()) {
        // A document written before `root` existed: the caller's root is the
        // only information there is.
        (true, true) => Err(usage_error("snapshot: the install root is empty")),
        (true, false) => Ok(absolute_clean(Path::new(caller))),
        (false, true) => Ok(absolute_clean(Path::new(snapshot_root))),
        (false, false) => {
            if !same_directory(caller, snapshot_root) {
                return Err(Error::new(
                    Code::StateDrift,
                    format!(
                        "snapshot: {:?} was captured from {snapshot_root:?}, which is not the \
                         requested root {caller:?}; refusing to restore into another directory",
                        snapshot.id
                    ),
                )
                .with_param("requested", caller)
                .with_param("root", snapshot_root)
                .with_param("snapshot", snapshot.id.clone()));
            }
            Ok(absolute_clean(Path::new(snapshot_root)))
        }
    }
}

/// True when two spellings name the same directory. Windows paths are
/// case-insensitive, so the comparison is too.
fn same_directory(left: &str, right: &str) -> bool {
    absolute_clean(Path::new(left))
        .display()
        .to_string()
        .eq_ignore_ascii_case(&absolute_clean(Path::new(right)).display().to_string())
}

fn absolute_clean(path: &Path) -> PathBuf {
    let clean = o824_model::path::clean(path);
    match std::path::absolute(&clean) {
        Ok(absolute) => o824_model::path::clean(&absolute),
        Err(_) => clean,
    }
}

/// One file to capture: root-relative (slash-separated) and absolute.
#[derive(Clone, Debug)]
struct Source {
    rel: String,
    abs: PathBuf,
}

/// Enumerates the ruled scope under `root`.
///
/// Only the exact file names are considered, so nothing is captured by walking
/// into `node_modules` or `vendor`.
fn collect(root: &Path) -> Result<Vec<Source>, Error> {
    let mut sources = Vec::with_capacity(PROFILE_FILE_NAMES.len() + INSTALL_FILE_NAMES.len());

    let profiles_dir = root.join(PROFILES_DIR_NAME);
    match fs::read_dir(&profiles_dir) {
        Ok(entries) => {
            let mut profiles: Vec<fs::DirEntry> = entries.flatten().collect();
            // ReadDir sorts by name in the reference implementation.
            profiles.sort_by_key(|entry| entry.file_name());
            for profile in profiles {
                if !profile
                    .file_type()
                    .map(|kind| kind.is_dir())
                    .unwrap_or(false)
                {
                    continue;
                }
                for name in PROFILE_FILE_NAMES {
                    let rel = format!(
                        "{PROFILES_DIR_NAME}/{}/{}",
                        profile.file_name().to_string_lossy(),
                        name
                    );
                    if let Some(source) = source_under(root, &rel)? {
                        sources.push(source);
                    }
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(file_error(
                "read the profiles directory",
                &profiles_dir,
                &error,
            ))
        }
    }

    for rel in INSTALL_FILE_NAMES {
        if let Some(source) = source_under(root, rel)? {
            sources.push(source);
        }
    }

    // Deterministic order keeps meta.json and the CLI's output stable.
    sources.sort_by(|left, right| left.rel.cmp(&right.rel));
    Ok(sources)
}

/// Resolves the slash-separated `rel` against `root` and reports whether it is
/// present.
///
/// A file that exists but cannot be inspected is an error: silently skipping it
/// would make the snapshot incomplete while still claiming to be complete.
fn source_under(root: &Path, rel: &str) -> Result<Option<Source>, Error> {
    let abs = root.join(rel.replace('/', "\\"));
    match regular_file(&abs) {
        Ok(true) => Ok(Some(Source {
            rel: rel.to_owned(),
            abs,
        })),
        Ok(false) => Ok(None),
        Err(error) => Err(file_error("inspect a snapshot target", &abs, &error)),
    }
}

/// Creates one empty snapshot directory and its `files\` subdirectory, retrying
/// on the astronomically unlikely name collision.
fn make_snapshot_dir(install_dir: &Path, now: Timestamp) -> Result<PathBuf, Error> {
    let prefix = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    for _ in 0..MAX_CREATE_ATTEMPTS {
        let dir = install_dir.join(format!("{prefix}-{}", random_hex(RANDOM_HEX_LEN)?));
        match fs::create_dir(&dir) {
            Ok(()) => {
                let files = dir.join(FILES_DIR_NAME);
                if let Err(error) = fs::create_dir(&files) {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(file_error(
                        "create the snapshot files directory",
                        &files,
                        &error,
                    ));
                }
                return Ok(dir);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(file_error("create the snapshot directory", &dir, &error)),
        }
    }
    Err(Error::new(
        Code::Internal,
        format!(
            "snapshot: cannot allocate a unique snapshot directory under {:?}",
            install_dir.display()
        ),
    )
    .with_param("dir", install_dir.display().to_string()))
}

/// Copies one source into the snapshot and proves the copy matches the source.
fn copy_into(dir: &Path, source: &Source) -> Result<FileEntry, Error> {
    let (source_hash, source_size) = hash_file(&source.abs)
        .map_err(|error| file_error("read the snapshot source", &source.abs, &error))?;
    let destination = dir.join(FILES_DIR_NAME).join(source.rel.replace('/', "\\"));
    copy_file(&source.abs, &destination)
        .map_err(|error| file_error("write the snapshot copy", &destination, &error))?;
    let (copy_hash, copy_size) = hash_file(&destination)
        .map_err(|error| file_error("read the snapshot copy", &destination, &error))?;
    if copy_hash != source_hash || copy_size != source_size {
        return Err(Error::new(
            Code::StateDrift,
            format!(
                "snapshot: the copy of {:?} does not match its source (sha256 {copy_hash}, want {source_hash})",
                source.rel
            ),
        )
        .with_param("rel", source.rel.clone())
        .with_param("sha256", copy_hash)
        .with_param("want", source_hash));
    }
    Ok(FileEntry {
        rel: source.rel.clone(),
        sha256: source_hash,
        size: source_size,
    })
}

/// Writes the snapshot document into `dir`.
///
/// It goes through a temporary file in the same directory and a rename, so a
/// reader never observes a partial `meta.json`.
fn write_meta(dir: &Path, snapshot: &Snapshot) -> Result<(), Error> {
    let mut data = serde_json::to_vec_pretty(snapshot).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("snapshot: cannot encode the snapshot document: {error}"),
        )
        .with_param("dir", dir.display().to_string())
        .with_param("error", error.to_string())
    })?;
    data.push(b'\n');

    let temporary = dir.join(format!(
        "{META_FILE_NAME}.tmp-{}-{}",
        std::process::id(),
        next_sequence()
    ));
    let write_result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&data)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(file_error(
            "write the snapshot document",
            &temporary,
            &error,
        ));
    }
    let meta_path = dir.join(META_FILE_NAME);
    if let Err(error) = fs::rename(&temporary, &meta_path) {
        let _ = fs::remove_file(&temporary);
        return Err(file_error(
            "publish the snapshot document",
            &meta_path,
            &error,
        ));
    }
    Ok(())
}

/// Deletes all but the newest [`RETENTION`] complete snapshots, never the one just
/// created. Incomplete directories are left alone: another process may still be
/// writing one.
fn prune(install_dir: &Path, keep: &str) -> Result<(), Error> {
    let snapshots = list_dir(install_dir)?;
    if snapshots.len() <= RETENTION {
        return Ok(());
    }
    for stale in &snapshots[RETENTION..] {
        if stale.id == keep {
            continue;
        }
        let dir = if stale.dir.is_empty() {
            install_dir.join(&stale.id)
        } else {
            PathBuf::from(&stale.dir)
        };
        fs::remove_dir_all(&dir)
            .map_err(|error| file_error("remove the expired snapshot", &dir, &error))?;
    }
    Ok(())
}

/// Reads the complete snapshots under `install_dir`, newest first.
fn list_dir(install_dir: &Path) -> Result<Vec<Snapshot>, Error> {
    let entries = match fs::read_dir(install_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(file_error("read the snapshot store", install_dir, &error)),
    };

    let mut snapshots = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let Ok(data) = fs::read(dir.join(META_FILE_NAME)) else {
            // No meta.json means the snapshot never finished: not a snapshot.
            continue;
        };
        let Ok(mut snapshot) = serde_json::from_slice::<Snapshot>(&data) else {
            continue;
        };
        // The directory name is the id and the location by construction, so the
        // listing is authoritative where a hand-edited document would not be.
        snapshot.id = entry.file_name().to_string_lossy().into_owned();
        snapshot.install_id = directory_name(install_dir);
        snapshot.dir = dir.display().to_string();
        snapshots.push(snapshot);
    }

    snapshots.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(snapshots)
}

/// Returns the current `rel -> FileEntry` set of the scope under `root`.
fn hash_scope(root: &Path) -> Result<BTreeMap<String, FileEntry>, Error> {
    let sources = collect(root)?;
    let mut current = BTreeMap::new();
    for source in sources {
        let (hash, size) = hash_file(&source.abs)
            .map_err(|error| file_error("read a snapshot target", &source.abs, &error))?;
        current.insert(
            source.rel.clone(),
            FileEntry {
                rel: source.rel,
                sha256: hash,
                size,
            },
        );
    }
    Ok(current)
}

/// True when the snapshot captured exactly the current set: the same rels, each
/// with the same sha256.
fn same_scope(snapshot: &Snapshot, current: &BTreeMap<String, FileEntry>) -> bool {
    if snapshot.files.len() != current.len() {
        return false;
    }
    let mut remaining: BTreeMap<&str, &FileEntry> = current
        .iter()
        .map(|(rel, entry)| (rel.as_str(), entry))
        .collect();
    for entry in &snapshot.files {
        let Some(found) = remaining.remove(entry.rel.as_str()) else {
            return false;
        };
        if found.sha256 != entry.sha256 {
            return false;
        }
    }
    remaining.is_empty()
}

/// The lowercase hex sha256 and the size of a file.
fn hash_file(path: &Path) -> io::Result<(String, i64)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size: i64 = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as i64;
    }
    Ok((hex(&hasher.finalize()), size))
}

/// Writes `source`'s bytes to `destination`, creating parent directories. The file
/// is flushed before it is closed so the copy is on disk when the call returns.
fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}

/// Resolves the slash-separated `rel` against `root` and refuses anything that
/// would leave `root`: an absolute path, a drive designator, or a `..` segment.
///
/// The rel path comes from `meta.json`, which a human can edit.
fn join_under(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return Err("empty relative path".to_owned());
    }
    if looks_absolute(rel) {
        return Err(format!("{rel:?} is absolute"));
    }
    let clean = o824_model::path::clean(Path::new(&rel.replace('/', "\\")));
    let clean_text = clean.display().to_string();
    if clean_text == "." || clean_text == ".." || clean_text.starts_with("..\\") {
        return Err(format!("{rel:?} escapes the root"));
    }
    if clean.is_absolute() {
        return Err(format!("{rel:?} escapes the root"));
    }
    Ok(root.join(clean))
}

/// True when `rel` starts with a separator or carries a Windows volume name,
/// regardless of slash direction.
fn looks_absolute(rel: &str) -> bool {
    if rel.starts_with('/') || rel.starts_with('\\') {
        return true;
    }
    let bytes = rel.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// True for a regular file; an absent path is `Ok(false)` and an uninspectable one
/// is an error.
fn regular_file(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Returns `n` hex characters derived from the clock and the process id.
///
/// The reference implementation uses a cryptographic source; here the value only
/// has to be unique among snapshots created moments apart by this machine. It is
/// not a security boundary, and the id is validated as a single path element
/// before it is ever used as a directory name.
fn random_hex(n: usize) -> Result<String, Error> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let seed = format!("{nanos}-{}-{}", std::process::id(), next_sequence());
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    Ok(hex(&hasher.finalize())[..n].to_owned())
}

fn next_sequence() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn directory_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Rejects an id that is not exactly one path element.
///
/// The id names a directory under the data root and is user-supplied through
/// `--install`, so it must never be able to escape.
fn validate_install_id(install_id: &str) -> Result<(), Error> {
    if install_id.is_empty() {
        return Err(usage_error("snapshot: the installation id is empty"));
    }
    if !is_single_path_element(install_id) {
        return Err(Error::new(
            Code::Usage,
            format!("snapshot: {install_id:?} is not a usable installation id"),
        )
        .with_param("installId", install_id));
    }
    Ok(())
}

/// Rejects a snapshot id that is not exactly one path element.
fn validate_snapshot_id(snapshot_id: &str) -> Result<(), Error> {
    if snapshot_id.is_empty() {
        return Err(usage_error("snapshot: the snapshot id is empty"));
    }
    if !is_single_path_element(snapshot_id) {
        return Err(Error::new(
            Code::Usage,
            format!("snapshot: {snapshot_id:?} is not a usable snapshot id"),
        )
        .with_param("snapshotId", snapshot_id));
    }
    Ok(())
}

/// Validates and cleans the directory a snapshot covers.
fn validate_scope_root(root: &Path) -> Result<PathBuf, Error> {
    if root.display().to_string().trim().is_empty() {
        return Err(usage_error("snapshot: the install root is empty"));
    }
    let clean_root = o824_model::path::clean(root);
    if !clean_root.is_dir() {
        return Err(Error::new(
            Code::Usage,
            format!(
                "snapshot: {:?} is not an installation directory",
                clean_root.display().to_string()
            ),
        )
        .with_param("root", clean_root.display().to_string()));
    }
    Ok(clean_root)
}

/// True when `value` can be used as exactly one path element.
fn is_single_path_element(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', ':', '\0'])
}

fn usage_error(message: &str) -> Error {
    Error::new(Code::Usage, message)
}

/// Reports a restore that could not prove it worked.
fn drift_error(snapshot: &Snapshot, entry: &FileEntry, operation: &str, cause: &str) -> Error {
    Error::new(
        Code::StateDrift,
        format!(
            "snapshot: cannot restore {:?} from {:?}: {operation}: {cause}",
            entry.rel, snapshot.id
        ),
    )
    .with_param("error", cause)
    .with_param("op", operation)
    .with_param("rel", entry.rel.clone())
    .with_param("snapshot", snapshot.id.clone())
}

/// Wraps a filesystem failure as a kernel error, keeping permission denials
/// distinct so callers can suggest a repair.
fn file_error(operation: &str, path: &Path, cause: &io::Error) -> Error {
    let code = if cause.kind() == io::ErrorKind::PermissionDenied {
        Code::Permission
    } else {
        Code::Internal
    };
    Error::new(
        code,
        format!(
            "snapshot: cannot {operation} {:?}: {cause}",
            path.display().to_string()
        ),
    )
    .with_param("error", cause.to_string())
    .with_param("path", path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn store() -> TempDir {
        TempDir::new("o824-snapshot-store-").expect("store")
    }

    #[test]
    fn a_snapshot_captures_the_ruled_files_only() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        let snapshot = create(store.path(), "i1", fixture.root()).expect("create");

        let rels: Vec<&str> = snapshot
            .files
            .iter()
            .map(|entry| entry.rel.as_str())
            .collect();
        assert_eq!(
            rels,
            [
                "profiles/desktop/cordis.patch.yml",
                "profiles/desktop/cordis.yml",
                "profiles/desktop/package.json",
                "profiles/desktop/pnpm-lock.yaml",
                "profiles/desktop/pnpm-workspace.yaml",
                "version",
            ]
        );
        assert_eq!(snapshot.install_version, o824_fixtures::DSH_VERSION);
        assert!(snapshot
            .root
            .eq_ignore_ascii_case(&fixture.root().display().to_string()));
    }

    #[test]
    fn a_snapshot_lists_newest_first_and_survives_a_second_create() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        let first = create(store.path(), "i1", fixture.root()).expect("first");
        let second = create(store.path(), "i1", fixture.root()).expect("second");
        let listed = list(store.path(), "i1").expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id);
        assert_eq!(listed[1].id, first.id);
    }

    #[test]
    fn a_restore_puts_the_bytes_back() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        let snapshot = create(store.path(), "i1", fixture.root()).expect("create");

        let patch = fixture.profile_dir().join("cordis.patch.yml");
        let original = fs::read(&patch).expect("read original");
        fs::write(&patch, "corrupted\n").expect("corrupt");
        restore(store.path(), &snapshot, fixture.root()).expect("restore");
        assert_eq!(fs::read(&patch).expect("read"), original);
    }

    #[test]
    fn a_tampered_snapshot_is_state_drift() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        let snapshot = create(store.path(), "i1", fixture.root()).expect("create");
        let captured = Path::new(&snapshot.dir)
            .join(FILES_DIR_NAME)
            .join("profiles")
            .join("desktop")
            .join("cordis.yml");
        fs::write(&captured, "[]\n[]\n").expect("tamper");

        let error = restore(store.path(), &snapshot, fixture.root()).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift);
        assert!(
            error.fallback.contains("snapshot file content"),
            "{}",
            error.fallback
        );
    }

    #[test]
    fn a_restore_into_another_root_is_refused() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let other = o824_fixtures::build("ok").expect("other");
        let store = store();
        let snapshot = create(store.path(), "i1", fixture.root()).expect("create");
        let error = restore(store.path(), &snapshot, other.root()).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift);
        assert!(
            error.fallback.contains("refusing to restore"),
            "{}",
            error.fallback
        );
    }

    #[test]
    fn create_if_changed_does_not_write_twice() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        let (first, created) =
            create_if_changed(store.path(), "i1", fixture.root()).expect("first");
        assert!(created);
        let (second, created) =
            create_if_changed(store.path(), "i1", fixture.root()).expect("second");
        assert!(!created);
        assert_eq!(first.id, second.id);
        assert_eq!(list(store.path(), "i1").expect("list").len(), 1);

        // A changed file creates a new snapshot. The ok fixture ships `[]\n` in
        // cordis.yml, so the change has to be something else.
        fs::write(fixture.profile_dir().join("cordis.yml"), "- id: changed\n").expect("write");
        let (_, created) = create_if_changed(store.path(), "i1", fixture.root()).expect("third");
        assert!(created);
    }

    #[test]
    fn retention_keeps_five_snapshots() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        for _ in 0..(RETENTION + 2) {
            create(store.path(), "i1", fixture.root()).expect("create");
        }
        assert_eq!(list(store.path(), "i1").expect("list").len(), RETENTION);
    }

    #[test]
    fn an_unfinished_snapshot_is_not_listed() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        create(store.path(), "i1", fixture.root()).expect("create");
        // A directory with no meta.json is a crashed create.
        fs::create_dir_all(
            store
                .path()
                .join("snapshots")
                .join("i1")
                .join("20260101T000000Z-dead"),
        )
        .expect("mkdir");
        assert_eq!(list(store.path(), "i1").expect("list").len(), 1);
    }

    #[test]
    fn a_missing_store_lists_nothing() {
        let store = store();
        assert!(list(store.path(), "i1").expect("list").is_empty());
    }

    #[test]
    fn an_id_that_could_escape_is_rejected() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        for bad in ["", ".", "..", "a/b", "a\\b", "c:stream"] {
            let error = create(store.path(), bad, fixture.root()).expect_err("must fail");
            assert_eq!(error.code, Code::Usage, "{bad:?}");
        }
    }

    #[test]
    fn a_root_that_is_not_a_directory_is_usage() {
        let store = store();
        let error =
            create(store.path(), "i1", &store.path().join("missing")).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
    }

    #[test]
    fn a_snapshot_round_trips_through_meta_json() {
        let fixture = o824_fixtures::build("ok").expect("fixture");
        let store = store();
        let snapshot = create(store.path(), "i1", fixture.root()).expect("create");
        let text = fs::read_to_string(Path::new(&snapshot.dir).join(META_FILE_NAME)).expect("read");
        assert!(text.contains("\"installId\": \"i1\""), "{text}");
        assert!(text.contains("\"createdAt\": \""), "{text}");
        assert!(text.ends_with("\n"));
        let restored: Snapshot = serde_json::from_str(&text).expect("parse");
        assert_eq!(restored.files, snapshot.files);
    }
}
