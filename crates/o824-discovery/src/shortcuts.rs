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
            if let Some(root) = root_of_link(&link) {
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

/// The installation root a shortcut names, when it names one.
fn root_of_link(link: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        let target = resolve_link(link)?;
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

/// Resolves one `.lnk` to its target path through the shell.
#[cfg(windows)]
fn resolve_link(link: &Path) -> Option<String> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::IPersistFile;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // SAFETY: COM is initialized on this thread for the duration of the call; the
    // returned interfaces are released by Rust's reference counting.
    unsafe {
        // A thread that is already initialized in another model still works here:
        // reading a link needs no message pump.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let shell_link: IShellLinkW =
            CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = shell_link.cast().ok()?;
        persist
            .Load(
                &HSTRING::from(link.as_os_str()),
                windows::Win32::System::Com::STGM_READ,
            )
            .ok()?;

        let mut buffer = vec![0u16; 1024];
        shell_link
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
    fn a_directory_without_shortcuts_contributes_nothing() {
        let temp = o824_fixtures::TempDir::new("o824-discovery-lnk-").expect("temp dir");
        std::fs::write(temp.path().join("notes.txt"), b"not a shortcut").expect("write");
        assert!(shortcut_roots(&[temp.path().to_path_buf()]).is_empty());
        assert!(shortcut_roots(&[temp.path().join("nope")]).is_empty());
    }
}
