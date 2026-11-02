// The process layer, kept in its own file so the raw-pointer work stays together and
// the rest of `machine` stays readable.

use std::collections::BTreeSet;
use std::path::Path;

use crate::{SourceRoot, EXE_NAME};

/// One process whose image is the host executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningHost {
    /// The process id; it is what a later `stop` would address.
    pub pid: u32,
    /// The full image path, as the system reports it.
    pub image: String,
}

/// Every running process whose image is named like the host executable.
///
/// This is the live-evidence layer: a directory the registry has forgotten is still
/// an installation if the host is running out of it. A process that cannot be opened
/// (an elevated one, a system one) is skipped rather than failing the scan — the
/// absence of evidence here is not evidence of absence.
#[cfg(windows)]
pub fn host_processes() -> Vec<RunningHost> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut hosts = Vec::new();
    // SAFETY: the snapshot handle is closed on every path below, and the entry struct
    // is initialized with its size before any call writes into it.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return hosts;
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = wide_to_string(&entry.szExeFile);
                if name.eq_ignore_ascii_case(EXE_NAME) {
                    if let Some(image) = image_path_of(entry.th32ProcessID) {
                        hosts.push(RunningHost {
                            pid: entry.th32ProcessID,
                            image,
                        });
                    }
                }
                entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    hosts
}

/// The directories the running host executables were started from, sorted and unique.
///
/// Sorted here rather than at the call site so the discovery order stays
/// machine-independent: two runs with the same processes produce the same list.
pub fn running_host_roots() -> Vec<String> {
    let mut roots: BTreeSet<String> = BTreeSet::new();
    for host in host_processes() {
        if let Some(parent) = Path::new(&host.image).parent() {
            if !parent.as_os_str().is_empty() {
                roots.insert(parent.display().to_string());
            }
        }
    }
    roots.into_iter().collect()
}

/// The process layer's contribution to discovery.
pub fn process_roots() -> Vec<SourceRoot> {
    running_host_roots()
        .into_iter()
        .map(|root| SourceRoot {
            root,
            registry_version: None,
            source: crate::machine::Source::Process,
        })
        .collect()
}

/// The full image path of one process, or `None` when it cannot be opened.
#[cfg(windows)]
fn image_path_of(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: the handle is closed below, and the buffer is owned by this function for
    // the duration of the call.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(handle);
        if result.is_err() {
            return None;
        }
        buffer.truncate(length as usize);
        Some(String::from_utf16_lossy(&buffer))
    }
}

/// Copies a fixed-size UTF-16 field out of a process entry, up to its NUL.
#[cfg(windows)]
fn wide_to_string(field: &[u16]) -> String {
    let end = field
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(field.len());
    String::from_utf16_lossy(&field[..end])
}
