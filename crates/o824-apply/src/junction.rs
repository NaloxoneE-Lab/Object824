//! Directory junctions, created natively.
//!
//! `mklink /J` is not an option for the kernel: a command line gives `cmd.exe`
//! another chance to re-interpret a path containing `&`, `|`, `<`, `>`, `^`, or
//! `%`. This module builds the reparse point itself with
//! `FSCTL_SET_REPARSE_POINT`, so no command line exists.
//!
//! The layout of a mount-point reparse buffer is fixed by Windows:
//!
//! ```text
//! [0:4]   uint32  ReparseTag          = IO_REPARSE_TAG_MOUNT_POINT
//! [4:6]   uint16  ReparseDataLength   = everything after this header
//! [6:8]   uint16  Reserved
//! [8:10]  uint16  SubstituteNameOffset
//! [10:12] uint16  SubstituteNameLength
//! [12:14] uint16  PrintNameOffset
//! [14:16] uint16  PrintNameLength
//! [16:...]        SubstituteName, then PrintName, both NUL-terminated UTF-16
//! ```
//!
//! The substitute name is the NT spelling (`\??\D:\...`) the object manager
//! follows; the print name is what Explorer and `read_link` show. Both buffers
//! carry their NUL terminator while the recorded lengths exclude it — the shape
//! `mklink` itself writes.

/// The fixed part of `REPARSE_DATA_BUFFER`.
const REPARSE_DATA_HEADER_SIZE: usize = 8;
/// The four `uint16` length fields that precede `PathBuffer` in a mount point.
const MOUNT_POINT_FIELDS_SIZE: usize = 8;
/// Where `PathBuffer` starts.
const MOUNT_POINT_DATA_OFFSET: usize = REPARSE_DATA_HEADER_SIZE + MOUNT_POINT_FIELDS_SIZE;
/// Turns a Win32 path into an NT object-manager path.
const NT_PATH_PREFIX: &str = r"\??\";

/// Writes the reparse buffer for a junction from `path` to `target`.
///
/// Split out from the ioctl so the buffer layout is testable without touching
/// the filesystem.
pub fn mount_point_buffer(target: &str) -> Vec<u8> {
    let substitute = to_utf16(&format!("{NT_PATH_PREFIX}{target}"));
    let print_name = to_utf16(target);
    // The buffers include the NUL terminator; the recorded lengths exclude it.
    let substitute_bytes = (substitute.len() - 1) * 2;
    let print_bytes = (print_name.len() - 1) * 2;
    let substitute_buffer = substitute.len() * 2;
    let print_buffer = print_name.len() * 2;
    // ReparseDataLength counts everything after the 8-byte header: the four
    // mount-point length fields plus the two NUL-terminated path buffers.
    let reparse_data_length = MOUNT_POINT_FIELDS_SIZE + substitute_buffer + print_buffer;

    let mut buffer = vec![0u8; MOUNT_POINT_DATA_OFFSET + substitute_buffer + print_buffer];
    buffer[0..4].copy_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer[4..6].copy_from_slice(&(reparse_data_length as u16).to_le_bytes());
    // 6..8 stays zero: Reserved.
    buffer[8..10].copy_from_slice(&0u16.to_le_bytes()); // SubstituteNameOffset
    buffer[10..12].copy_from_slice(&(substitute_bytes as u16).to_le_bytes());
    buffer[12..14].copy_from_slice(&(substitute_buffer as u16).to_le_bytes()); // PrintNameOffset
    buffer[14..16].copy_from_slice(&(print_bytes as u16).to_le_bytes());

    let mut cursor = MOUNT_POINT_DATA_OFFSET;
    for unit in &substitute {
        buffer[cursor..cursor + 2].copy_from_slice(&unit.to_le_bytes());
        cursor += 2;
    }
    for unit in &print_name {
        buffer[cursor..cursor + 2].copy_from_slice(&unit.to_le_bytes());
        cursor += 2;
    }
    buffer
}

/// UTF-16 code units with no terminator.
fn to_utf16(text: &str) -> Vec<u16> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    // The buffer layout carries a NUL terminator for each name.
    units.push(0);
    units
}

/// `IO_REPARSE_TAG_MOUNT_POINT`.
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

#[cfg(windows)]
pub use platform::{set_junction, FSCTL_SET_REPARSE_POINT_VALUE};

#[cfg(windows)]
mod platform {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_MODE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    use super::mount_point_buffer;

    /// `FSCTL_SET_REPARSE_POINT`: the control code that installs a reparse point.
    pub const FSCTL_SET_REPARSE_POINT_VALUE: u32 = 0x0009_00A4;

    /// Turns `path` — which must already exist as an empty directory — into a
    /// directory junction pointing at `target`.
    ///
    /// `target` must be absolute; the caller resolves it.
    pub fn set_junction(path: &Path, target: &Path) -> std::io::Result<()> {
        let buffer = mount_point_buffer(&target.display().to_string());
        let wide = wide_path(path);

        // SAFETY: every pointer below is either a local that outlives the call or
        // is owned by the handle, which is closed exactly once before returning.
        // The buffer is a valid mount-point reparse buffer produced by
        // `mount_point_buffer`, sized to match.
        unsafe {
            let handle: HANDLE = CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(
                    FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0,
                ),
                None,
            )
            .map_err(|error| std::io::Error::other(error.to_string()))?;

            let mut returned = 0u32;
            let result = DeviceIoControl(
                handle,
                FSCTL_SET_REPARSE_POINT_VALUE,
                Some(buffer.as_ptr() as *const _),
                buffer.len() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            );
            let _ = CloseHandle(handle);
            result.map_err(|error| std::io::Error::other(error.to_string()))
        }
    }

    /// A NUL-terminated UTF-16 path.
    fn wide_path(path: &Path) -> Vec<u16> {
        OsStr::new(&path.display().to_string())
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_has_the_mount_point_layout() {
        let buffer = mount_point_buffer(r"D:\target");
        assert_eq!(
            u32::from_le_bytes(buffer[0..4].try_into().unwrap()),
            0xA000_0003
        );
        // Everything after the 8-byte header.
        assert_eq!(
            u16::from_le_bytes(buffer[4..6].try_into().unwrap()) as usize,
            buffer.len() - 8
        );
        assert_eq!(u16::from_le_bytes(buffer[6..8].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(buffer[8..10].try_into().unwrap()), 0);

        // The substitute name is the NT spelling, NUL-terminated, and its length
        // excludes the terminator.
        let substitute_len = u16::from_le_bytes(buffer[10..12].try_into().unwrap()) as usize;
        assert_eq!(substitute_len, (r"\??\D:\target".len()) * 2);
        let substitute_start = MOUNT_POINT_DATA_OFFSET;
        let substitute: Vec<u16> =
            buffer[substitute_start..substitute_start + substitute_len + 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
        assert_eq!(
            String::from_utf16(&substitute[..substitute.len() - 1]).unwrap(),
            r"\??\D:\target"
        );

        // The print name follows it.
        let print_offset = u16::from_le_bytes(buffer[12..14].try_into().unwrap()) as usize;
        let print_len = u16::from_le_bytes(buffer[14..16].try_into().unwrap()) as usize;
        assert_eq!(print_offset, substitute_len + 2);
        assert_eq!(print_len, r"D:\target".len() * 2);
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_created_natively_resolves_to_its_target() {
        use o824_fixtures::TempDir;

        let root = TempDir::new("o824-junction-").expect("temp");
        let target = root.path().join("target");
        std::fs::create_dir(&target).expect("target");
        std::fs::write(target.join("marker.txt"), b"hello").expect("marker");

        let link = root.path().join("link");
        std::fs::create_dir(&link).expect("empty link directory");
        set_junction(&link, &target).expect("set the reparse point");

        // The junction resolves through to the target's contents.
        let marker = link.join("marker.txt");
        assert_eq!(
            std::fs::read(&marker).expect("read through the link"),
            b"hello"
        );

        // And it is a reparse point, not a copy.
        let metadata = std::fs::symlink_metadata(&link).expect("metadata");
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        assert_ne!(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT, 0);

        // Removing the link leaves the target alone.
        std::fs::remove_dir(&link).expect("remove the link");
        assert!(target.join("marker.txt").is_file());
    }
}
