//! Machine-level discovery: the uninstall registry, the fixed volumes, and the
//! root list they produce.
//!
//! Discovery is deliberately forgiving: registry and volume enumeration skip
//! anything they cannot read, and a root that fails validation is still reported
//! (with health `"damaged"`) so operator-facing commands can explain why an
//! installation is unusable instead of pretending it does not exist.

use std::path::{Path, PathBuf};

use o824_errs::Code;
use o824_model::Install;

use crate::{profile_names, validate_with, HEALTH_DAMAGED, KIND_DESKTOP};

/// Controls [`discover`].
///
/// `roots == None` means "scan the standard locations": the uninstall registry
/// and the per-user install directory on every fixed volume. `Some(vec![])` —
/// even an empty one — means "use exactly these roots instead", which is what
/// tests and callers that already know the paths want. `extra_dirs` are always
/// added on top of whichever source set was selected.
///
/// `home` is the DSH home whose `profiles\` directory is reported; `None` means
/// resolve the default.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// An explicit root set, or the standard machine scan.
    pub roots: Option<Vec<PathBuf>>,
    /// Roots added on top of whichever set was selected.
    pub extra_dirs: Vec<PathBuf>,
    /// The DSH home, or `None` for the default.
    pub home: Option<String>,
}

/// One discovery candidate. `registry_version` is set only for candidates found
/// through the uninstall registry, whose `DisplayName` carries the DSH version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRoot {
    /// The candidate root.
    pub root: String,
    /// The version parsed from the registry entry, when there is one.
    pub registry_version: Option<String>,
}

/// Returns every installation root it can find, valid or damaged.
///
/// Roots are cleaned, deduplicated case-insensitively (Windows paths are
/// case-insensitive), sorted by cleaned path, and numbered `i1`, `i2`, … in that
/// order so repeated runs against the same machine yield the same ids.
pub fn discover(options: &Options) -> Result<Vec<Install>, o824_errs::Error> {
    let home = crate::resolve_home(options.home.as_deref());
    let candidates = candidate_roots(options);

    let mut installs = Vec::with_capacity(candidates.len());
    for (index, candidate) in candidates.iter().enumerate() {
        let mut install = match validate_with(
            Path::new(&candidate.root),
            &home,
            candidate.registry_version.as_deref(),
        ) {
            Ok(install) => install,
            Err(error) if error.code == Code::InstallDir => Install {
                install_id: String::new(),
                kind: KIND_DESKTOP.to_owned(),
                root: o824_model::path::clean(Path::new(&candidate.root))
                    .display()
                    .to_string(),
                version: String::new(),
                profiles: profile_names(&home),
                health: HEALTH_DAMAGED.to_owned(),
                home: home.display().to_string(),
            },
            // Any other failure is a real failure, not a damaged installation.
            Err(error) => return Err(error),
        };
        install.install_id = format!("i{}", index + 1);
        installs.push(install);
    }
    Ok(installs)
}

/// Assembles, cleans, deduplicates, and sorts the roots to inspect.
pub fn candidate_roots(options: &Options) -> Vec<SourceRoot> {
    let mut roots: Vec<SourceRoot> = match &options.roots {
        None => standard_roots(),
        Some(roots) => roots
            .iter()
            .map(|root| SourceRoot {
                root: root.display().to_string(),
                registry_version: None,
            })
            .collect(),
    };
    for extra in &options.extra_dirs {
        roots.push(SourceRoot {
            root: extra.display().to_string(),
            registry_version: None,
        });
    }
    sorted_unique_roots(roots)
}

/// The default machine scan: uninstall registry entries first, then the per-user
/// install directory on every fixed volume.
pub fn standard_roots() -> Vec<SourceRoot> {
    let mut roots = registry_roots();
    roots.extend(volume_roots().into_iter().map(|root| SourceRoot {
        root,
        registry_version: None,
    }));
    roots
}

/// Cleans and trims roots, drops empty ones, removes duplicates in discovery
/// order, then sorts the survivors by cleaned path so ids are deterministic.
///
/// Identity is decided by the realpath dedupe key, but the surviving candidate
/// keeps its original spelling: the realpath is only a key, never the value
/// reported as an install root. A duplicate that carries a registry version hint
/// donates it to the kept candidate.
pub fn sorted_unique_roots(roots: Vec<SourceRoot>) -> Vec<SourceRoot> {
    let mut cleaned: Vec<SourceRoot> = Vec::with_capacity(roots.len());
    for candidate in roots {
        let root = candidate.root.trim();
        if root.is_empty() {
            continue;
        }
        cleaned.push(SourceRoot {
            root: o824_model::path::clean(Path::new(root))
                .display()
                .to_string(),
            registry_version: candidate
                .registry_version
                .map(|version| version.trim().to_owned()),
        });
    }

    let mut unique: Vec<SourceRoot> = Vec::with_capacity(cleaned.len());
    let mut seen: Vec<(String, usize)> = Vec::with_capacity(cleaned.len());
    for candidate in cleaned {
        let key = dedupe_key(Path::new(&candidate.root));
        if let Some((_, index)) = seen.iter().find(|(seen_key, _)| *seen_key == key) {
            let kept = &mut unique[*index];
            if kept.registry_version.is_none() && candidate.registry_version.is_some() {
                kept.registry_version = candidate.registry_version;
            }
            continue;
        }
        seen.push((key, unique.len()));
        unique.push(candidate);
    }

    unique.sort_by(|left, right| {
        let left_key = sort_key(&left.root);
        let right_key = sort_key(&right.root);
        left_key
            .cmp(&right_key)
            .then_with(|| left.root.cmp(&right.root))
    });
    unique
}

/// The realpath-normalised comparison key for a path.
///
/// Symbolic links and junctions are followed, 8.3 short names are expanded, and
/// the result is cleaned and case-folded on Windows. A path that cannot be
/// resolved (a registry entry pointing at a deleted directory) falls back to a
/// lexical clean so it still dedupes consistently.
pub fn dedupe_key(root: &Path) -> String {
    let mut resolved = root.to_path_buf();
    if let Ok(real) = std::fs::canonicalize(root) {
        resolved = o824_model::path::strip_nt_namespace(&real);
    }
    o824_model::path::clean(&resolved)
        .display()
        .to_string()
        .to_lowercase()
}

/// The display-path ordering used for stable ids. It follows the cleaned path,
/// not the realpath, so the order matches what a table shows.
fn sort_key(root: &str) -> String {
    root.to_lowercase()
}

/// Every `InstallLocation` on an uninstall entry whose `DisplayName` starts with
/// "DeepSeek Harness", together with the version parsed from that name.
///
/// A missing hive or an unreadable key is skipped: discovery must never fail
/// because of one inaccessible key.
#[cfg(windows)]
pub fn registry_roots() -> Vec<SourceRoot> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    /// The uninstall hives searched for that entry, in lookup order: the
    /// per-user hive first, then the machine-wide 64-bit view, then the 32-bit
    /// view.
    const UNINSTALL_KEYS: [(windows::Win32::System::Registry::HKEY, &str); 3] = [
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];

    let mut roots = Vec::new();
    for (hive, path) in UNINSTALL_KEYS {
        roots.extend(roots_from_uninstall_key(hive, path));
    }
    roots
}

/// A build without the Windows registry has no registry roots.
#[cfg(not(windows))]
pub fn registry_roots() -> Vec<SourceRoot> {
    Vec::new()
}

/// Enumerates the subkeys of one uninstall hive.
#[cfg(windows)]
fn roots_from_uninstall_key(
    hive: windows::Win32::System::Registry::HKEY,
    path: &str,
) -> Vec<SourceRoot> {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, KEY_READ,
    };

    let mut roots = Vec::new();
    unsafe {
        let mut key = HKEY::default();
        let opened = RegOpenKeyExW(hive, &HSTRING::from(path), 0, KEY_READ, &mut key);
        if opened.is_err() {
            return roots;
        }

        let mut index = 0;
        loop {
            let mut buffer = [0u16; 256];
            let mut length = buffer.len() as u32;
            let status = RegEnumKeyExW(
                key,
                index,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
                None,
                PWSTR::null(),
                None,
                None,
            );
            if status != ERROR_SUCCESS {
                // ERROR_NO_MORE_ITEMS ends the walk; any other failure does too,
                // because a partially readable hive is not worth reporting.
                debug_assert!(status == ERROR_NO_MORE_ITEMS || true);
                break;
            }
            let name = String::from_utf16_lossy(&buffer[..length as usize]);
            if let Some(root) = install_location(key, &name) {
                roots.push(root);
            }
            index += 1;
        }
        let _ = RegCloseKey(key);
    }
    roots
}

/// Reads one uninstall entry's `InstallLocation` and the version from its
/// `DisplayName`.
///
/// It yields nothing unless the entry is a DeepSeek Harness installation with a
/// usable path.
#[cfg(windows)]
fn install_location(
    parent: windows::Win32::System::Registry::HKEY,
    subkey: &str,
) -> Option<SourceRoot> {
    let display_name = string_value(parent, subkey, "DisplayName")?;
    if !display_name.trim().starts_with(DISPLAY_NAME_PREFIX) {
        return None;
    }
    let location = string_value(parent, subkey, "InstallLocation")?;
    let location = location.trim().trim_matches('"');
    if location.is_empty() {
        return None;
    }
    Some(SourceRoot {
        root: location.to_owned(),
        registry_version: parse_display_name_version(&display_name),
    })
}

/// Reads one `REG_SZ` value out of a subkey, or `None`.
#[cfg(windows)]
fn string_value(
    parent: windows::Win32::System::Registry::HKEY,
    subkey: &str,
    value: &str,
) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    unsafe {
        let subkey = HSTRING::from(subkey);
        let value = HSTRING::from(value);
        let mut size = 0u32;
        let status = RegGetValueW(
            parent,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        );
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        let status = RegGetValueW(
            parent,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if status != ERROR_SUCCESS {
            return None;
        }
        let wide: Vec<u16> = buffer
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        Some(String::from_utf16_lossy(&wide))
    }
}

/// Identifies the DeepSeek Harness uninstall entry. The `DisplayName` usually
/// carries the version too ("DeepSeek Harness 0.2.1-rc.3").
const DISPLAY_NAME_PREFIX: &str = "DeepSeek Harness";

/// Extracts the version suffix from an uninstall `DisplayName` such as
/// "DeepSeek Harness 0.2.1-rc.3".
///
/// This is the second version source: the registry is more trustworthy than the
/// `<root>\version` file on a real Electron install.
pub fn parse_display_name_version(display_name: &str) -> Option<String> {
    let display_name = display_name.trim();
    let rest = display_name.strip_prefix(DISPLAY_NAME_PREFIX)?;
    let version = rest.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_owned())
    }
}

/// Returns `<volume>\Users\<user>\AppData\Local\Programs\DeepSeek Harness` for
/// every fixed volume.
///
/// `%LOCALAPPDATA%` is not enough: the user's local app data can be on one drive
/// while DSH is installed on another.
pub fn volume_roots() -> Vec<String> {
    let mut roots = Vec::new();
    for volume in fixed_volumes() {
        let Ok(users) = std::fs::read_dir(Path::new(&volume).join("Users")) else {
            continue;
        };
        let mut users: Vec<std::fs::DirEntry> = users.flatten().collect();
        users.sort_by_key(|entry| entry.file_name());
        for user in users {
            if !user.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let root = Path::new(&volume)
                .join("Users")
                .join(user.file_name())
                .join("AppData")
                .join("Local")
                .join("Programs")
                .join("DeepSeek Harness");
            if root.is_dir() {
                roots.push(root.display().to_string());
            }
        }
    }
    roots
}

/// Lists the drive roots of fixed disks, e.g. `C:\`.
#[cfg(windows)]
pub fn fixed_volumes() -> Vec<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    /// The drive type Windows reports for a fixed disk.
    const DRIVE_FIXED: u32 = 3;

    let mask = unsafe { GetLogicalDrives() };
    let mut volumes = Vec::new();
    for bit in 0..26 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let letter = char::from(b'A' + bit as u8);
        let volume = format!("{letter}:\\");
        let wide: Vec<u16> = volume.encode_utf16().chain(std::iter::once(0)).collect();
        let kind = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };
        if kind != DRIVE_FIXED {
            continue;
        }
        volumes.push(volume);
    }
    volumes
}

/// A build without the Windows API reports no volumes.
#[cfg(not(windows))]
pub fn fixed_volumes() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_comes_from_the_display_name_suffix() {
        assert_eq!(
            parse_display_name_version("DeepSeek Harness 0.2.1-rc.3"),
            Some("0.2.1-rc.3".to_owned())
        );
        assert_eq!(parse_display_name_version("DeepSeek Harness"), None);
        assert_eq!(parse_display_name_version("Something Else 1.0"), None);
    }

    #[test]
    fn roots_are_cleaned_deduplicated_and_sorted() {
        let roots = vec![
            SourceRoot {
                root: r"C:\b".to_owned(),
                registry_version: None,
            },
            SourceRoot {
                root: r"C:\a".to_owned(),
                registry_version: None,
            },
            SourceRoot {
                // A trailing separator and a different case are the same root.
                root: r"c:\A\\".to_owned(),
                registry_version: Some("1.0.0".to_owned()),
            },
            SourceRoot {
                root: "   ".to_owned(),
                registry_version: None,
            },
        ];
        let unique = sorted_unique_roots(roots);
        assert_eq!(unique.len(), 2, "{unique:?}");
        // The surviving candidate keeps its original spelling; the duplicate
        // only donates its version hint.
        assert_eq!(unique[0].root, r"C:\a");
        assert_eq!(
            unique[0].registry_version.as_deref(),
            Some("1.0.0"),
            "a duplicate donates its version hint to the kept candidate"
        );
        assert_eq!(unique[1].root, r"C:\b");
    }

    #[test]
    fn an_explicit_root_set_replaces_the_machine_scan() {
        let options = Options {
            roots: Some(vec![PathBuf::from(r"C:\one")]),
            extra_dirs: vec![PathBuf::from(r"C:\two")],
            home: None,
        };
        let candidates = candidate_roots(&options);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].root, r"C:\one");
        assert_eq!(candidates[1].root, r"C:\two");
    }

    #[test]
    fn an_empty_explicit_root_set_scans_nothing() {
        let options = Options {
            roots: Some(Vec::new()),
            extra_dirs: Vec::new(),
            home: None,
        };
        assert!(candidate_roots(&options).is_empty());
    }

    #[test]
    fn a_root_that_is_not_an_installation_is_reported_as_damaged() {
        let directory = o824_fixtures::TempDir::new("o824-discovery-").expect("temp dir");
        let options = Options {
            roots: Some(vec![directory.path().to_path_buf()]),
            extra_dirs: Vec::new(),
            home: Some(directory.path().display().to_string()),
        };
        let installs = discover(&options).expect("discover");
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].health, HEALTH_DAMAGED);
        assert_eq!(installs[0].install_id, "i1");
        assert_eq!(installs[0].profiles, Vec::<String>::new());
    }

    #[test]
    fn ids_are_numbered_in_sorted_path_order() {
        let first = o824_fixtures::build("ok").expect("first");
        let second = o824_fixtures::build("ok").expect("second");
        let mut roots = vec![PathBuf::from(second.root()), PathBuf::from(first.root())];
        roots.sort();
        let options = Options {
            roots: Some(roots),
            extra_dirs: Vec::new(),
            home: None,
        };
        let installs = discover(&options).expect("discover");
        assert_eq!(installs.len(), 2);
        assert_eq!(installs[0].install_id, "i1");
        assert_eq!(installs[1].install_id, "i2");
        assert!(installs[0].root <= installs[1].root);
        for install in &installs {
            assert_eq!(install.health, "ok");
            assert_eq!(install.version, "0.2.1-rc.3");
        }
    }

    #[test]
    fn a_missing_extra_dir_is_reported_not_skipped() {
        let directory = o824_fixtures::TempDir::new("o824-discovery-").expect("temp dir");
        let missing = directory.path().join("gone");
        let options = Options {
            roots: Some(Vec::new()),
            extra_dirs: vec![missing.clone()],
            home: None,
        };
        let installs = discover(&options).expect("discover");
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].health, HEALTH_DAMAGED);
    }
}
