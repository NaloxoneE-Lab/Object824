// The shortcut layer: a user may start DSH from a desktop or start-menu shortcut that
// points at an installation the registry has never heard of.
//
// The link is read through `IShellLink`, not by parsing the `.lnk` format: the format
// has several generations of path storage, and the shell is the only thing that reads
// all of them correctly. A link that cannot be resolved contributes nothing.

use std::path::{Path, PathBuf};

use crate::machine::{Source, SourceRoot};
use crate::EXE_NAME;

/// The directories shortcuts are looked for in by default.
///
/// Per-user first, then all-users: a per-user shortcut is the one the user made, and
/// it outranks an installer's. Missing directories are simply absent.
pub fn default_shortcut_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(profile) = std::env::var("USERPROFILE")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        dirs.push(PathBuf::from(&profile).join("Desktop"));
    }
    if let Some(appdata) = std::env::var("APPDATA")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        dirs.push(
            PathBuf::from(&appdata)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    if let Some(program_data) = std::env::var("ProgramData")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        dirs.push(
            PathBuf::from(&program_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    dirs
}

/// The roots the shortcuts in `dirs` point at.
///
/// Only the top level of each directory is read: an installer puts its shortcut there,
/// and walking a start-menu tree would cost more than it finds.
pub fn shortcut_roots(dirs: &[PathBuf]) -> Vec<SourceRoot> {
    let mut roots = Vec::new();
    // COM is initialized once and one shell-link object is reused for every file: a
    // fresh `CoCreateInstance` per shortcut costs about ten milliseconds each, which
    // turned a 2 ms layer into a 148 ms one on a machine with a populated start menu.
    let Some(resolver) = LinkResolver::new() else {
        return roots;
    };
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut links: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .map(|extension| extension.eq_ignore_ascii_case("lnk"))
                    .unwrap_or(false)
            })
            .collect();
        links.sort();
        for link in links {
            if !worth_resolving(&link) {
                continue;
            }
            if let Some(root) = root_of_link(&resolver, &link) {
                roots.push(SourceRoot {
                    root,
                    registry_version: None,
                    source: Source::Shortcut,
                });
            }
        }
    }
    roots
}

/// Whether a shortcut is worth the shell's time.
///
/// Resolving a link costs about two milliseconds — the shell reads the file, expands
/// whatever environment variables the installer used, and may look at the target — and
/// a populated start menu holds dozens of links that have nothing to do with DSH. So
/// the file's bytes are searched for the product name first: a link to the host
/// executable has to name it somewhere, while a link to a browser does not.
///
/// The file *name* counts too, because an installer may name the shortcut after the
/// product while storing the target behind an environment variable.
fn worth_resolving(link: &Path) -> bool {
    const NEEDLE: &str = "deepseek harness";
    if let Some(name) = link
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
    {
        if name.contains("deepseek") {
            return true;
        }
    }
    let Ok(bytes) = std::fs::read(link) else {
        return false;
    };
    contains_ascii_case_insensitive(&bytes, NEEDLE)
        || contains_utf16_case_insensitive(&bytes, NEEDLE)
}

/// ASCII substring search, case-insensitive.
fn contains_ascii_case_insensitive(haystack: &[u8], needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

/// Lowercases one UTF-16 unit when it is ASCII; the product name is ASCII.
fn ascii_lower(unit: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

/// UTF-16LE substring search, case-insensitive: the link format stores paths as wide
/// strings.
fn contains_utf16_case_insensitive(haystack: &[u8], needle: &str) -> bool {
    let needle: Vec<u16> = needle.encode_utf16().map(ascii_lower).collect();
    if needle.is_empty() || haystack.len() < needle.len() * 2 {
        return false;
    }
    let units: Vec<u16> = haystack
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    units.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(&needle)
            .all(|(a, b)| ascii_lower(*a) == *b)
    })
}

/// The installation root a shortcut names, when it names one.
fn root_of_link(resolver: &LinkResolver, link: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        let target = resolver.resolve(link)?;
        let target_path = Path::new(&target);
        let name = target_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if name
            .as_deref()
            .map(|name| name.eq_ignore_ascii_case(EXE_NAME))
            .unwrap_or(false)
        {
            return Some(target_path.parent()?.display().to_string());
        }
        // A shortcut may point at the install directory itself rather than the exe.
        if target_path.is_dir() {
            return Some(target_path.display().to_string());
        }
        None
    }
    #[cfg(not(windows))]
    {
        let _ = link;
        None
    }
}

/// A shell-link object that can resolve many `.lnk` files.
///
/// Holding one object and reusing it is what keeps the layer cheap: the per-link cost
/// is a `Load` and a `GetPath`, not an object construction.
#[cfg(windows)]
pub struct LinkResolver {
    link: windows::Win32::UI::Shell::IShellLinkW,
    persist: windows::Win32::System::Com::IPersistFile,
}

#[cfg(windows)]
impl LinkResolver {
    /// Initializes COM on this thread and creates the resolver.
    pub fn new() -> Option<LinkResolver> {
        use windows::core::Interface;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

        // SAFETY: COM is initialized for this thread; the interfaces are reference
        // counted. A thread already initialized in another model still works here,
        // because reading a link needs no message pump.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist: IPersistFile = link.cast().ok()?;
            Some(LinkResolver { link, persist })
        }
    }

    /// Resolves one `.lnk` to its target path.
    fn resolve(&self, link: &Path) -> Option<String> {
        use windows::core::HSTRING;
        use windows::Win32::System::Com::STGM_READ;

        // SAFETY: both interfaces are alive for the whole call, and the buffer is owned
        // by this function.
        unsafe {
            self.persist
                .Load(&HSTRING::from(link.as_os_str()), STGM_READ)
                .ok()?;
            let mut buffer = vec![0u16; 1024];
            self.link
                .GetPath(&mut buffer, std::ptr::null_mut(), 0)
                .ok()?;
            let end = buffer
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(buffer.len());
            if end == 0 {
                return None;
            }
            Some(String::from_utf16_lossy(&buffer[..end]))
        }
    }
}

/// A resolver that finds nothing, for platforms and situations without COM.
#[cfg(not(windows))]
pub struct LinkResolver;

#[cfg(not(windows))]
impl LinkResolver {
    fn new() -> Option<LinkResolver> {
        Some(LinkResolver)
    }

    fn resolve(&self, _link: &Path) -> Option<String> {
        None
    }
}

/// Creates a shortcut, for tests. Uses the same interface the reader uses, so a test
/// that passes proves the reader handles what Windows itself writes.
#[cfg(all(test, windows))]
pub(crate) fn write_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // SAFETY: same contract as `resolve_link`.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let shell_link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
        shell_link
            .SetPath(&HSTRING::from(target.as_os_str()))
            .map_err(|e| e.to_string())?;
        let persist: IPersistFile = shell_link.cast().map_err(|e| e.to_string())?;
        persist
            .Save(&HSTRING::from(link.as_os_str()), true)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_pointing_at_a_root_is_followed() {
        let fixture = o824_fixtures::build("ok").expect("build the ok fixture");
        let temp = o824_fixtures::TempDir::new("o824-discovery-lnk-").expect("temp dir");
        let link = temp.path().join("DeepSeek Harness.lnk");
        write_shortcut(&link, &fixture.root().join(crate::EXE_NAME)).expect("write a shortcut");

        let roots = shortcut_roots(&[temp.path().to_path_buf()]);
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(
            roots[0].root,
            o824_model::path::clean(fixture.root())
                .display()
                .to_string()
        );
        assert_eq!(roots[0].source, Source::Shortcut);
    }

    #[test]
    fn the_prefilter_recognises_the_product_name_in_both_encodings() {
        // The link format stores paths as wide strings, but a hand-written or older
        // link can carry an ANSI path; both spellings have to pass the filter.
        assert!(contains_ascii_case_insensitive(
            br"C:\Tools\DeepSeek Harness\DeepSeek Harness.exe",
            "deepseek harness"
        ));
        let wide: Vec<u8> = r"C:\Tools\DeepSeek Harness"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert!(contains_utf16_case_insensitive(&wide, "deepseek harness"));
        assert!(!contains_ascii_case_insensitive(
            br"C:\Firefox\firefox.exe",
            "deepseek harness"
        ));
        assert!(!contains_utf16_case_insensitive(
            &wide[..4],
            "deepseek harness"
        ));
    }

    #[test]
    fn a_directory_without_shortcuts_contributes_nothing() {
        let temp = o824_fixtures::TempDir::new("o824-discovery-lnk-").expect("temp dir");
        std::fs::write(temp.path().join("notes.txt"), b"not a shortcut").expect("write");
        assert!(shortcut_roots(&[temp.path().to_path_buf()]).is_empty());
        assert!(shortcut_roots(&[temp.path().join("nope")]).is_empty());
    }
}
