//! O824's small, machine-readable state: the registry of known installations
//! and the data-root location rule.
//!
//! The file is written atomically — a temporary file in the destination
//! directory, flushed, closed, then renamed over the old one — so a crash or a
//! power loss can never leave a truncated `state.json` behind. Readers therefore
//! observe either the complete old file or the complete new one.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use o824_errs::{Code, Error};
use serde::{Deserialize, Serialize};

/// The environment variable that overrides the data root. It is part of the CLI
/// contract and is honoured before any default.
pub const DATA_ROOT_ENV: &str = "O824_DATA_ROOT";

/// The directory name of the data root under `%LOCALAPPDATA%` or the user home.
const DATA_ROOT_DIR_NAME: &str = "o824";

/// The state-file name inside the data root.
pub const STATE_FILE_NAME: &str = "state.json";

/// The state-file record of one known installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRef {
    /// The installation root.
    pub root: String,
    /// The installation shape.
    pub kind: String,
}

/// A process this kernel started and is responsible for.
///
/// The image and the start time are the fingerprint: a pid is reused, so "the process
/// with pid 1234" is not the same claim tomorrow as it is now, and killing on pid alone
/// is how a tool murders something it never started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRecord {
    /// The installation this process serves.
    pub install_id: String,
    /// The process id.
    pub pid: u32,
    /// The port it listened on, when it is a server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// The DSH home it was started with.
    pub home: String,
    /// The image it runs, as the system reported it.
    pub image: String,
    /// The OS creation time, in the form Windows reports it.
    ///
    /// Together with `image` this is the process's identity: a pid is reused, but the
    /// same pid never carries the same creation time twice. Without it, a record can
    /// only say "something with this pid exists", which is not enough to kill.
    pub created: u64,
    /// When it was started, for a human reading the state file.
    pub started_at: String,
    /// The file its stdout and stderr go to.
    pub log_path: String,
}

/// The persisted state document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// One record per installation id.
    ///
    /// The reference implementation marshals a nil Go map as `null`; a state
    /// file written by either implementation must load in the other, so `null`
    /// and a missing key both read as "no installations".
    #[serde(default, deserialize_with = "deserialize_installs")]
    pub installs: BTreeMap<String, InstallRef>,
    /// One record per process this kernel started, by installation id.
    ///
    /// The reference implementation knows nothing about processes; an unknown key is
    /// ignored by it, and a file it wrote has no such key here. Both directions read
    /// as "nothing is running", which is the truth for a file that never had the field.
    #[serde(default, deserialize_with = "deserialize_processes")]
    pub processes: BTreeMap<String, ProcessRecord>,
}

impl State {
    /// Records one installation under `id`.
    pub fn record(
        &mut self,
        id: impl Into<String>,
        root: impl Into<String>,
        kind: impl Into<String>,
    ) {
        self.installs.insert(
            id.into(),
            InstallRef {
                root: root.into(),
                kind: kind.into(),
            },
        );
    }
}

impl State {
    /// Records one started process under the installation it serves.
    pub fn remember_process(&mut self, record: ProcessRecord) {
        self.processes.insert(record.install_id.clone(), record);
    }

    /// Forgets the process serving `install_id`, if there is one.
    pub fn forget_process(&mut self, install_id: &str) -> Option<ProcessRecord> {
        self.processes.remove(install_id)
    }
}

/// Reads `null` — which is what a Go implementation writes for an empty map —
/// as an empty map.
fn deserialize_installs<'de, D>(deserializer: D) -> Result<BTreeMap<String, InstallRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let found: Option<BTreeMap<String, InstallRef>> = Option::deserialize(deserializer)?;
    Ok(found.unwrap_or_default())
}

/// Reads a missing or `null` process map as empty, like the install map.
fn deserialize_processes<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, ProcessRecord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let found: Option<BTreeMap<String, ProcessRecord>> = Option::deserialize(deserializer)?;
    Ok(found.unwrap_or_default())
}

/// Resolves the per-user data root.
///
/// Precedence: `$O824_DATA_ROOT`, then `%LOCALAPPDATA%\o824`, then
/// `<home>\o824`, then the user's home directory as a last resort. The `home`
/// parameter is only consulted when `%LOCALAPPDATA%` is empty.
///
/// The location is deliberately per-user: cross-Windows-user exclusion is a
/// known P0 gap.
pub fn data_root(explicit: Option<&str>) -> PathBuf {
    if let Some(root) = explicit.map(str::trim).filter(|root| !root.is_empty()) {
        return PathBuf::from(root);
    }
    if let Some(root) = env_value(DATA_ROOT_ENV) {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Some(local) = env_value("LOCALAPPDATA") {
        let trimmed = local.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(DATA_ROOT_DIR_NAME);
        }
    }
    if let Some(home) = std::env::home_dir() {
        return home.join(DATA_ROOT_DIR_NAME);
    }
    PathBuf::from(DATA_ROOT_DIR_NAME)
}

/// The state file inside a data root.
pub fn state_path(data_root: &Path) -> PathBuf {
    data_root.join(STATE_FILE_NAME)
}

/// Reads the state file at `path`.
///
/// A missing file is not an error: the caller gets an empty state so first runs
/// need no special case. A file that exists but cannot be parsed is
/// `E_STATE_DRIFT`; the caller must not silently start from empty state, because
/// that would lose the installation registry.
pub fn load(path: &Path) -> Result<State, Error> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => return Err(file_error("read state file", path, &error)),
    };
    serde_json::from_slice(&data).map_err(|error| {
        Error::new(
            Code::StateDrift,
            format!("state file {path:?} is corrupt: {error}"),
        )
        // Keys in the order the reference implementation's map marshals them.
        .with_param("error", error.to_string())
        .with_param("path", path.display().to_string())
    })
}

/// Writes `state` to `path` atomically.
///
/// The document goes to a temporary file in the destination directory, is
/// flushed to disk, closed, and then renamed over `path`. The rename is the
/// commit point, so a reader never sees a partial document.
pub fn save(path: &Path, state: &State) -> Result<(), Error> {
    let Some(directory) = path.parent() else {
        return Err(file_error(
            "derive the state directory",
            path,
            &io::Error::other("the state path has no parent directory"),
        ));
    };
    if let Err(error) = fs::create_dir_all(directory) {
        return Err(file_error("create state directory", directory, &error));
    }

    let mut data = serde_json::to_vec_pretty(state).map_err(|error| {
        Error::new(
            Code::Internal,
            format!("state: cannot encode state for {path:?}: {error}"),
        )
        .with_param("error", error.to_string())
        .with_param("path", path.display().to_string())
    })?;
    data.push(b'\n');

    // The temporary file must live in the destination directory so the final
    // rename stays on one volume.
    let temporary = temporary_path(path);
    let write_result = write_and_flush(&temporary, &data);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(file_error("replace state file", path, &error));
    }
    Ok(())
}

/// Writes `data` to `path`, flushes it to disk, and closes it.
fn write_and_flush(path: &Path, data: &[u8]) -> Result<(), Error> {
    let mut file = fs::File::create(path)
        .map_err(|error| file_error("create temporary state file", path, &error))?;
    file.write_all(data)
        .map_err(|error| file_error("write temporary state file", path, &error))?;
    file.sync_all()
        .map_err(|error| file_error("flush temporary state file", path, &error))?;
    Ok(())
}

/// Builds a unique temporary path next to `path`.
fn temporary_path(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state.json".to_owned());
    let temporary = format!("{name}.tmp-{}-{sequence}", std::process::id());
    match path.parent() {
        Some(directory) => directory.join(temporary),
        None => PathBuf::from(temporary),
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Wraps a filesystem failure as a kernel error, keeping `E_PERMISSION` for
/// access denials so callers can suggest a repair.
fn file_error(operation: &str, path: &Path, cause: &io::Error) -> Error {
    let code = if cause.kind() == io::ErrorKind::PermissionDenied {
        Code::Permission
    } else {
        Code::Internal
    };
    Error::new(code, format!("state: cannot {operation} {path:?}: {cause}"))
        .with_param("error", cause.to_string())
        .with_param("path", path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new("o824-state-").expect("temp dir")
    }

    #[test]
    fn an_explicit_root_wins() {
        assert_eq!(data_root(Some(r"D:\data")), PathBuf::from(r"D:\data"));
    }

    #[test]
    fn the_default_is_under_local_app_data() {
        let root = data_root(None);
        assert!(root.ends_with(DATA_ROOT_DIR_NAME), "{}", root.display());
    }

    #[test]
    fn a_missing_state_file_is_an_empty_state() {
        let dir = temp_dir();
        let state = load(&state_path(dir.path())).expect("load");
        assert!(state.installs.is_empty());
    }

    #[test]
    fn a_state_file_round_trips() {
        let dir = temp_dir();
        let path = state_path(dir.path());
        let mut state = State::default();
        state.record("i1", r"C:\harness", "desktop");
        state.record("i2", r"D:\other", "desktop");
        save(&path, &state).expect("save");

        let restored = load(&path).expect("load");
        assert_eq!(restored, state);

        let text = fs::read_to_string(&path).expect("read");
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"installs\""));
        assert!(text.contains(r"C:\\harness"));
    }

    /// The reference implementation writes `"installs": null` for an empty
    /// registry; a file written by either side has to load in the other.
    #[test]
    fn a_null_installs_map_reads_as_empty() {
        let dir = temp_dir();
        let path = dir.path().join("state.json");
        fs::write(&path, "{\"installs\": null}\n").expect("write");
        let state = load(&path).expect("load");
        assert!(state.installs.is_empty());
    }

    #[test]
    fn a_corrupt_state_file_is_state_drift() {
        let dir = temp_dir();
        let path = dir.path().join("state.json");
        fs::write(&path, "{not json").expect("write");
        let error = load(&path).expect_err("must fail");
        assert_eq!(error.code, Code::StateDrift);
        assert!(error.fallback.contains("is corrupt"), "{}", error.fallback);
        assert!(error.params.contains_key("path"));
        assert!(error.params.contains_key("error"));
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = temp_dir();
        let path = state_path(dir.path());
        save(&path, &State::default()).expect("save");
        let names: Vec<String> = fs::read_dir(dir.path())
            .expect("read dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [STATE_FILE_NAME.to_owned()], "{names:?}");
    }

    #[test]
    fn saving_over_an_existing_file_replaces_it() {
        let dir = temp_dir();
        let path = state_path(dir.path());
        let mut first = State::default();
        first.record("i1", r"C:\a", "desktop");
        save(&path, &first).expect("save");

        let mut second = State::default();
        second.record("i2", r"C:\b", "desktop");
        save(&path, &second).expect("save");

        let restored = load(&path).expect("load");
        assert_eq!(restored, second);
    }

    #[test]
    fn a_state_file_without_processes_reads_as_nothing_running() {
        let dir = temp_dir();
        let path = state_path(dir.path());
        // What the reference implementation writes: installations, and no notion of a
        // process at all.
        std::fs::write(
            &path,
            r#"{"installs":{"local-1":{"root":"D:\\x","kind":"desktop"}}}"#,
        )
        .expect("write");

        let state = load(&path).expect("load");
        assert_eq!(state.installs.len(), 1);
        assert!(
            state.processes.is_empty(),
            "nothing running is the truth here"
        );
    }

    #[test]
    fn a_null_process_map_reads_as_empty() {
        let dir = temp_dir();
        let path = state_path(dir.path());
        std::fs::write(&path, r#"{"installs":null,"processes":null}"#).expect("write");
        let state = load(&path).expect("load");
        assert!(state.processes.is_empty());
    }

    #[test]
    fn a_process_record_round_trips() {
        let dir = temp_dir();
        let path = state_path(dir.path());
        let mut state = State::default();
        state.remember_process(ProcessRecord {
            install_id: "i2".to_owned(),
            pid: 4321,
            port: Some(21846),
            home: r"D:\home".to_owned(),
            image: r"C:\npm\node.exe".to_owned(),
            created: 133_700_000_000_000_000,
            started_at: "2026-11-03T02:00:00Z".to_owned(),
            log_path: r"D:\data\logs\i2.log".to_owned(),
        });
        save(&path, &state).expect("save");

        let loaded = load(&path).expect("load");
        assert_eq!(loaded.processes["i2"].pid, 4321);
        assert_eq!(loaded.processes["i2"].port, Some(21846));
        let mut loaded = loaded;
        assert_eq!(
            loaded.forget_process("i2").map(|record| record.pid),
            Some(4321)
        );
        assert!(loaded.processes.is_empty());
    }
}
