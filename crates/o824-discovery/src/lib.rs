//! Locates DeepSeek Harness installations on this machine and validates their
//! three-part desktop layout.
//!
//! Discovery is deliberately forgiving: a root that fails validation is still
//! reported (with health `"damaged"`) so operator-facing commands can explain
//! why an installation is unusable instead of pretending it does not exist.
//!
//! Milestone M0 implements the part the fixture acceptance needs — validating
//! one known root, reading the version out of `resources\app.asar`, and listing
//! the profiles under a DSH home. The registry/volume scan and the realpath
//! dedupe of `Discover` arrive in M1.
//!
//! The version is *not* read from `<root>\version`: on a real Electron install
//! that file holds Electron's runtime version (44.7.0 on the reference machine),
//! while the DSH version lives in `package.json` inside the asar. The fixture
//! trees keep the two deliberately different so this mistake cannot pass.

#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use o824_errs::{Code, Error};
use o824_model::Install;
use serde::Deserialize;

/// The desktop application entry point.
pub const EXE_NAME: &str = "DeepSeek Harness.exe";
/// The plain-text marker at the install root; the last-resort version source.
pub const VERSION_FILE_NAME: &str = "version";
/// The packaged application archive, relative to the root.
pub const ASAR_REL_PATH: &str = "resources\\app.asar";
/// The profile directory inside the DSH home.
pub const PROFILES_DIR_NAME: &str = "profiles";

/// The only install shape P0 knows how to validate.
pub const KIND_DESKTOP: &str = "desktop";
/// Marks a tree that passed [`validate`].
pub const HEALTH_OK: &str = "ok";
/// Marks a discovered root that failed [`validate`].
pub const HEALTH_DAMAGED: &str = "damaged";

/// O824's own override for the DSH home; it wins over DSH's variable (§19.6).
pub const HOME_ENV: &str = "O824_HOME";
/// DSH's own home variable.
pub const DSH_HOME_ENV: &str = "DSH_HOME";
/// The directory name of the DSH home under the user profile.
pub const HOME_DIR_NAME: &str = ".dsh";

/// Bounds the asar header we are willing to read. Real headers are a few
/// hundred kilobytes.
const MAX_ASAR_HEADER_SIZE: u32 = 64 << 20;
/// Bounds the `package.json` payload read out of an asar.
const MAX_PACKAGE_JSON_SIZE: i64 = 1 << 20;

/// Resolves the DSH home directory.
///
/// The first non-empty value wins: `explicit` (the `--home` flag), `O824_HOME`,
/// `DSH_HOME`, then `%USERPROFILE%\.dsh`.
pub fn resolve_home(explicit: Option<&str>) -> PathBuf {
    for candidate in [
        explicit,
        env_value(HOME_ENV).as_deref(),
        env_value(DSH_HOME_ENV).as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            return clean_path(Path::new(trimmed));
        }
    }
    if let Some(profile) = env_value("USERPROFILE") {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            return clean_path(Path::new(trimmed)).join(HOME_DIR_NAME);
        }
    }
    if let Some(home) = std::env::home_dir() {
        if !home.as_os_str().is_empty() {
            return home.join(HOME_DIR_NAME);
        }
    }
    PathBuf::from(HOME_DIR_NAME)
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Lists the directories under `<home>\profiles`, sorted by name.
///
/// The result is always empty rather than absent: `Install.profiles` has no
/// `omitempty`, and a missing array would render as `"profiles": null`.
pub fn profile_names(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let Ok(entries) = fs::read_dir(home.join(PROFILES_DIR_NAME)) else {
        return names;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

/// Checks that `root` is a complete three-part desktop installation: the app
/// executable, `resources\app.asar`, and a version (from `package.json` inside
/// the asar, then the `<root>\version` file).
///
/// Any missing part is `E_INSTALL_DIR`. The returned install has health `"ok"`
/// and the default DSH home; its id is left empty for discovery to assign.
pub fn validate(root: &Path) -> Result<Install, Error> {
    let clean = clean_path(root);
    let home = resolve_home(None);
    let exe = clean.join(EXE_NAME);
    let asar = clean.join("resources").join("app.asar");

    let mut missing: Vec<String> = Vec::with_capacity(3);
    if !is_regular_file(&exe) {
        missing.push(EXE_NAME.to_owned());
    }
    if !is_regular_file(&asar) {
        missing.push(ASAR_REL_PATH.to_owned());
    }
    let version = read_version(&clean, None);
    if version.is_empty() {
        missing.push(VERSION_FILE_NAME.to_owned());
    }
    if !missing.is_empty() {
        let display = clean.display();
        return Err(Error::new(
            Code::InstallDir,
            format!(
                "install: {display:?} is not a DeepSeek Harness installation (missing {})",
                missing.join(", ")
            ),
        )
        .with_param("root", display.to_string())
        .with_param("missing", missing));
    }

    Ok(Install {
        install_id: String::new(),
        kind: KIND_DESKTOP.to_owned(),
        root: clean.display().to_string(),
        version,
        profiles: profile_names(&home),
        health: HEALTH_OK.to_owned(),
        home: home.display().to_string(),
    })
}

/// Returns the DSH version, trying the authoritative source first: the
/// `package.json` inside `app.asar`, then the `<root>\version` file (which on a
/// real Electron install holds Electron's version).
pub fn read_version(root: &Path, registry_version: Option<&str>) -> String {
    if let Some(version) = parse_asar_version(&root.join("resources").join("app.asar")) {
        return version;
    }
    if let Some(hint) = registry_version {
        let trimmed = hint.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    if let Ok(bytes) = fs::read(root.join(VERSION_FILE_NAME)) {
        if let Ok(text) = String::from_utf8(bytes) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    String::new()
}

/// Reads `package.json` out of an asar archive and returns its version.
///
/// This is a best-effort read: any malformed or unexpected archive yields
/// `None`. Layout (all integers little-endian): `uint32` pickle size (4),
/// `uint32` header-pickle size, the header pickle, then file data. The header
/// pickle starts with its payload size and then the JSON string length.
pub fn parse_asar_version(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;

    let mut size_buffer = [0u8; 8];
    file.read_exact(&mut size_buffer).ok()?;
    let header_size = u32::from_le_bytes([
        size_buffer[4],
        size_buffer[5],
        size_buffer[6],
        size_buffer[7],
    ]);
    if !(8..=MAX_ASAR_HEADER_SIZE).contains(&header_size) {
        return None;
    }

    let mut header_pickle = vec![0u8; header_size as usize];
    file.read_exact(&mut header_pickle).ok()?;

    let header = if header_pickle.len() >= 8 {
        let json_len = u32::from_le_bytes([
            header_pickle[4],
            header_pickle[5],
            header_pickle[6],
            header_pickle[7],
        ]);
        if i64::from(json_len) > header_pickle.len() as i64 - 8 {
            return None;
        }
        let start = 8;
        let end = 8 + json_len as usize;
        &header_pickle[start..end]
    } else {
        &header_pickle[..]
    };
    let header = trim_trailing_nul(header);

    let archive: AsarArchive = serde_json::from_slice(header).ok()?;
    let entry = archive.files.get("package.json")?;
    if entry.unpacked || entry.size <= 0 || entry.size > MAX_PACKAGE_JSON_SIZE {
        return None;
    }
    let offset: i64 = entry.offset.parse().ok()?;
    if offset < 0 {
        return None;
    }

    let mut data = vec![0u8; entry.size as usize];
    // File data starts after the two size fields and the whole header pickle.
    let data_start = 8 + i64::from(header_size) + offset;
    file.seek(SeekFrom::Start(data_start as u64)).ok()?;
    file.read_exact(&mut data).ok()?;

    #[derive(Deserialize)]
    struct PackageJson {
        #[serde(default)]
        version: String,
    }
    let package: PackageJson = serde_json::from_slice(&data).ok()?;
    let version = package.version.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_owned())
    }
}

/// The subset of an asar header the reader needs.
#[derive(Deserialize)]
struct AsarArchive {
    #[serde(default)]
    files: std::collections::HashMap<String, AsarFile>,
}

/// One entry in an asar header's `files` map.
#[derive(Deserialize)]
struct AsarFile {
    #[serde(default)]
    size: i64,
    #[serde(default)]
    offset: String,
    #[serde(default)]
    unpacked: bool,
}

fn trim_trailing_nul(mut bytes: &[u8]) -> &[u8] {
    while let [rest @ .., 0] = bytes {
        bytes = rest;
    }
    bytes
}

fn is_regular_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// Normalises a path the way the reference implementation's `filepath.Clean`
/// does for the shapes this kernel produces: `.` components are dropped, `..`
/// pops one component, redundant separators disappear, and an empty result is
/// `.`.
pub fn clean_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !cleaned.pop() {
                    cleaned.push(OsString::from(".."));
                }
            }
            other => cleaned.push(other.as_os_str()),
        }
    }
    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_drops_dot_and_parent_components() {
        assert_eq!(
            clean_path(Path::new("C:\\a\\.\\b")),
            PathBuf::from("C:\\a\\b")
        );
        assert_eq!(
            clean_path(Path::new("C:\\a\\b\\..\\c")),
            PathBuf::from("C:\\a\\c")
        );
        assert_eq!(clean_path(Path::new("")), PathBuf::from("."));
        assert_eq!(clean_path(Path::new("a//b")), PathBuf::from("a\\b"));
    }

    #[test]
    fn missing_tree_is_an_install_dir_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = validate(dir.path()).expect_err("an empty directory is not an install");
        assert_eq!(error.code, Code::InstallDir);
        assert!(error.fallback.contains(EXE_NAME));
        assert!(error.fallback.contains(ASAR_REL_PATH));
        assert!(error.fallback.contains(VERSION_FILE_NAME));
        assert!(error.params.contains_key("root"));
        assert!(error.params.contains_key("missing"));
    }

    #[test]
    fn ok_fixture_validates_as_a_clean_install() {
        let fixture = o824_fixtures::build("ok").expect("build the ok fixture");
        let install = validate(fixture.root()).expect("the ok tree must validate");
        assert_eq!(install.health, HEALTH_OK);
        assert_eq!(install.version, o824_fixtures::DSH_VERSION);
        assert_eq!(install.kind, KIND_DESKTOP);
        assert_eq!(install.profiles, vec!["desktop".to_owned()]);
    }

    #[test]
    fn version_comes_from_the_asar_not_the_version_file() {
        let fixture = o824_fixtures::build("ok").expect("build the ok fixture");
        // The version file deliberately holds Electron's version.
        let marker = fs::read_to_string(fixture.root().join(VERSION_FILE_NAME)).expect("version");
        assert_eq!(marker.trim(), o824_fixtures::RUNTIME_VERSION);
        assert_ne!(marker.trim(), o824_fixtures::DSH_VERSION);
        let install = validate(fixture.root()).expect("validate");
        assert_eq!(install.version, o824_fixtures::DSH_VERSION);
    }

    #[test]
    fn half_install_has_no_asar_and_is_rejected() {
        let fixture = o824_fixtures::build("half-install").expect("build the half-install fixture");
        assert!(!fixture.root().join("resources").join("app.asar").exists());
        let error = validate(fixture.root()).expect_err("an install without an asar is damaged");
        assert_eq!(error.code, Code::InstallDir);
    }

    #[test]
    fn truncated_asar_yields_no_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.asar");
        fs::write(&path, b"not an asar").expect("write");
        assert_eq!(parse_asar_version(&path), None);
    }
}
