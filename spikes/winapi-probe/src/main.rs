//! Probe: can `windows` 0.58 build and link on this `windows-gnu` host, and what
//! are the exact signatures of the registry and volume calls discovery needs?

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};

fn main() {
    let drives = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    println!("logical drives mask = {drives:#x}");

    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let roots = roots_from(hive, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall");
        for (name, location) in roots {
            if name.starts_with("DeepSeek Harness") {
                println!("{name:40} -> {location}");
            }
        }
    }
}

fn roots_from(hive: HKEY, path: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(hive, &HSTRING::from(path), 0, KEY_READ, &mut key).0 != ERROR_SUCCESS.0
        {
            return found;
        }
        let mut index = 0u32;
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
            if status.0 == ERROR_NO_MORE_ITEMS.0 || status.0 != ERROR_SUCCESS.0 {
                break;
            }
            let name = String::from_utf16_lossy(&buffer[..length as usize]);
            if let Some(display) = string_value(key, &name, "DisplayName") {
                let location = string_value(key, &name, "InstallLocation").unwrap_or_default();
                found.push((display, location));
            }
            index += 1;
        }
        let _ = RegCloseKey(key);
    }
    found
}

fn string_value(parent: HKEY, subkey: &str, value: &str) -> Option<String> {
    unsafe {
        let mut size = 0u32;
        let status = RegGetValueW(
            parent,
            &HSTRING::from(subkey),
            &HSTRING::from(value),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        );
        if status.0 != ERROR_SUCCESS.0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size as usize];
        let status = RegGetValueW(
            parent,
            &HSTRING::from(subkey),
            &HSTRING::from(value),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if status.0 != ERROR_SUCCESS.0 {
            return None;
        }
        let wide: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        Some(String::from_utf16_lossy(&wide))
    }
}
