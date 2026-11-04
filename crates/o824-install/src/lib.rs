//! Where the launcher lives once it is installed, and how the system is told about it.
//!
//! This crate exists because installing is an operating-system operation, and the crates that
//! describe the program (`o824-cli` among them) forbid `unsafe` outright. Keeping the registry
//! calls here means that rule stays true everywhere else.
//!
//! The model is a per-user installation: `%LOCALAPPDATA%\o824`, one Start Menu shortcut, one
//! uninstall entry under `HKCU`. No administrator rights are needed, and none are requested.

// Denied rather than forbidden: the two registry calls need it.
#![deny(unsafe_code)]

use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};

/// The directory name under `%LOCALAPPDATA%`.
pub const DIRECTORY_NAME: &str = "o824";

/// The two files an installation consists of.
pub const FILES: [&str; 2] = ["o824.exe", "o824-gui.exe"];

/// Where the uninstall entry lives under `HKEY_CURRENT_USER`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\O824";

/// `%LOCALAPPDATA%\o824`.
pub fn default_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DIRECTORY_NAME)
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\O824.lnk`.
pub fn shortcut_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|appdata| {
            appdata
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("O824.lnk")
        })
}

/// The values a per-user uninstall entry carries.
///
/// Separated from the registry call so the content can be asserted without touching a machine:
/// a wrong `UninstallString` is the difference between "remove it from Settings" and an entry
/// that never goes away.
pub fn uninstall_values(install_dir: &Path, version: &str) -> Vec<(&'static str, String)> {
    vec![
        ("DisplayName", "O824".to_owned()),
        ("DisplayVersion", version.to_owned()),
        ("Publisher", "NaloxoneE-Lab".to_owned()),
        ("InstallLocation", install_dir.display().to_string()),
        (
            "UninstallString",
            format!(
                "\"{}\" uninstall --yes",
                install_dir.join("o824.exe").display()
            ),
        ),
        (
            "DisplayIcon",
            install_dir.join("o824-gui.exe").display().to_string(),
        ),
        // No repair and no modify: there is nothing for either to do, and offering them would
        // only produce buttons that fail.
        ("NoModify", "1".to_owned()),
        ("NoRepair", "1".to_owned()),
    ]
}

/// Creates the Start Menu shortcut, and returns where it went.
///
/// A `.lnk` is a COM object. Driving `WScript.Shell` through the system PowerShell is smaller
/// and far less error-prone than writing the binary format by hand, and a shortcut is a
/// convenience: callers treat a failure as a warning, not as a failed installation.
pub fn make_shortcut(target: &Path) -> Result<PathBuf, Error> {
    let Some(path) = shortcut_path() else {
        return Err(Error::new(
            Code::Internal,
            "install: APPDATA is not set, so there is no Start Menu".to_owned(),
        ));
    };
    let working = target.parent().unwrap_or(target);
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.Description = 'O824 launcher'; \
         $s.Save()",
        path.display(),
        target.display(),
        working.display()
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|error| {
            Error::new(
                Code::Internal,
                format!("install: cannot run PowerShell for the shortcut: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(Error::new(
            Code::Internal,
            format!(
                "install: the shortcut was not created: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(path)
}

/// Downloads one URL to one path.
///
/// `URLDownloadToFileW` is the platform's own downloader: it is already installed, it handles
/// redirects and proxy settings the way the rest of Windows does, and it avoids adding a TLS
/// stack to a launcher that only ever fetches release files from one host.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn download(url: &str, to: &Path) -> Result<(), Error> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Error::new(
                Code::Permission,
                format!(
                    "download: cannot create {:?}: {error}",
                    parent.display().to_string()
                ),
            )
        })?;
    }
    // urlmon is a COM component: calling it on a thread that never initialised COM fails with
    // INET_E_DOWNLOAD_FAILURE, which looks exactly like a broken network. Initialise for the
    // duration of the call, and only uninitialise when this call is the one that did it.
    // SAFETY: no pointers; the balance of CoInitializeEx/CoUninitialize is decided by its own
    // return value.
    let initialised = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let owned = initialised.is_ok() || initialised == windows::Win32::Foundation::S_FALSE;
    let status = unsafe {
        URLDownloadToFileW(
            None,
            &HSTRING::from(url),
            &HSTRING::from(to.display().to_string()),
            0,
            None,
        )
    };
    if owned {
        unsafe { CoUninitialize() };
    }
    status.map_err(|error| Error::new(Code::Network, format!("download: {url} failed: {error}")))
}

#[cfg(not(windows))]
pub fn download(url: &str, _to: &Path) -> Result<(), Error> {
    Err(Error::new(
        Code::Network,
        format!("download: this platform has no downloader wired up ({url})"),
    ))
}

/// Writes the per-user uninstall entry, and returns the key it wrote.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn write_uninstall_entry(install_dir: &Path, version: &str) -> Result<String, Error> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let values = uninstall_values(install_dir, version);
    // SAFETY: every buffer handed to the registry is owned by this function, the strings are
    // NUL-terminated, and the lengths are the byte counts the API documents.
    unsafe {
        let mut key = HKEY::default();
        let created = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(UNINSTALL_KEY),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        );
        if created != ERROR_SUCCESS {
            return Err(Error::new(
                Code::Permission,
                format!("install: cannot create the removal entry ({created:?})"),
            ));
        }
        for (name, value) in &values {
            let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            let status = RegSetValueExW(
                key,
                &HSTRING::from(*name),
                0,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    wide.as_ptr() as *const u8,
                    wide.len() * 2,
                )),
            );
            if status != ERROR_SUCCESS {
                let _ = RegCloseKey(key);
                return Err(Error::new(
                    Code::Permission,
                    format!("install: cannot write the removal entry value {name} ({status:?})"),
                ));
            }
        }
        let _ = RegCloseKey(key);
    }
    Ok(UNINSTALL_KEY.to_owned())
}

#[cfg(not(windows))]
pub fn write_uninstall_entry(_install_dir: &Path, _version: &str) -> Result<String, Error> {
    Err(Error::new(
        Code::Internal,
        "install: this platform has no removal entry to write".to_owned(),
    ))
}

/// Removes the per-user uninstall entry, ignoring a key that was never there.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn remove_uninstall_entry() {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegDeleteKeyW, HKEY_CURRENT_USER};

    // SAFETY: a hive and an owned key path; nothing outlives the call.
    unsafe {
        let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY));
    }
}

#[cfg(not(windows))]
pub fn remove_uninstall_entry() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uninstall_entry_points_at_this_installation() {
        let dir = Path::new(r"C:\Users\someone\AppData\Local\o824");
        let values = uninstall_values(dir, "0.2.0");
        let find = |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };

        assert_eq!(find("DisplayName"), "O824");
        assert_eq!(find("DisplayVersion"), "0.2.0");
        assert_eq!(find("InstallLocation"), dir.display().to_string());
        // Quoted, so a path with spaces still runs; and it names our own uninstall verb.
        assert_eq!(
            find("UninstallString"),
            format!("\"{}\" uninstall --yes", dir.join("o824.exe").display())
        );
        assert!(
            find("UninstallString").starts_with('"'),
            "the path is quoted"
        );
        assert!(find("DisplayIcon").ends_with("o824-gui.exe"));
    }

    #[test]
    fn the_default_directory_is_under_local_app_data() {
        let directory = default_directory();
        assert_eq!(
            directory.file_name().and_then(|name| name.to_str()),
            Some(DIRECTORY_NAME)
        );
    }

    #[test]
    fn the_shortcut_is_a_start_menu_file() {
        let Some(path) = shortcut_path() else {
            return; // no APPDATA in this environment; nothing to assert
        };
        assert!(
            path.ends_with("Start Menu\\Programs\\O824.lnk")
                || path.ends_with("Start Menu/Programs/O824.lnk")
        );
    }

    #[test]
    fn both_programs_are_part_of_an_installation() {
        assert_eq!(FILES.len(), 2);
        assert!(FILES.contains(&"o824.exe"));
        assert!(FILES.contains(&"o824-gui.exe"));
    }
}
