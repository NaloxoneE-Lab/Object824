//! The exclusive, machine-scoped lock that serialises every O824 write
//! operation.
//!
//! The lock file lives in the data root and is held with an operating-system
//! file lock, so a second O824 process — or a second thread — cannot write at the
//! same time. The holder also stamps its pid into a sidecar file, which is what
//! lets [`acquire`] answer `E_BUSY` with the process to wait for.
//!
//! # One documented difference from the reference implementation
//!
//! The Go version opens the lock file with share mode 0 (`CreateFileW`), which
//! refuses a *second open* — including one from the same process. This version
//! takes a `LockFileEx`-backed lock through the standard library's
//! [`File::try_lock`], which refuses a second *lock* from another process. Both
//! give cross-process exclusion, which is what the contract needs; they differ
//! only when one process tries to take the lock twice at the same time, which the
//! kernel never does (it takes the lock once per mutating command). The trade is
//! deliberate: no `unsafe`, and no dependency on the Windows API surface.

#![forbid(unsafe_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};

/// The lock file created inside the data root.
///
/// It is part of the on-disk contract: another O824 process (or a human) can find
/// the lock without reading this crate.
pub const LOCK_FILE_NAME: &str = "o824.lock";

/// The sidecar that publishes the current holder's pid.
///
/// The lock itself is not readable by a waiting process, so this is what lets
/// `E_BUSY` name the process to wait for.
pub const PID_FILE_NAME: &str = "o824.lock.pid";

/// A held lock. It must be released with [`Handle::release`]; dropping it
/// releases the lock too.
#[derive(Debug)]
pub struct Handle {
    file: Option<File>,
    pid_path: PathBuf,
    released: bool,
}

/// Takes the exclusive lock under `data_root`.
///
/// The data root is created if it does not exist. The holder stamps its pid into
/// `<dataRoot>\o824.lock.pid` and into the lock file itself, so a contender can
/// be told which process to wait for. The pid is `0` when the sidecar is missing
/// or unreadable — for example after a crash that bypassed release.
pub fn acquire(data_root: &Path) -> Result<Handle, Error> {
    fs::create_dir_all(data_root)
        .map_err(|error| io_error("create data root", data_root, &error))?;
    let path = data_root.join(LOCK_FILE_NAME);
    let pid_path = data_root.join(PID_FILE_NAME);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| io_error("open lock file", &path, &error))?;

    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(Error::new(
                Code::Busy,
                "Another O824 process is already writing to this data root. \
                 Wait for it to finish and try again.",
            )
            // Keys in the order the reference implementation's map marshals them.
            .with_param("path", path.display().to_string())
            .with_param("pid", read_pid(&pid_path)));
        }
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(io_error("lock lock file", &path, &error));
        }
    }

    let mut handle = Handle {
        file: Some(file),
        pid_path: pid_path.clone(),
        released: false,
    };
    if let Err(error) = handle.stamp_pid() {
        // Do not leak the OS lock when stamping the pid fails.
        let _ = handle.release();
        return Err(io_error("write lock file", &path, &error));
    }
    if let Err(error) = write_pid_file(&pid_path) {
        // Without the sidecar a contender's E_BUSY could not name this process.
        // That is part of the lock contract, so surface the failure instead of
        // silently degrading diagnostics.
        let _ = handle.release();
        return Err(io_error("write lock pid file", &pid_path, &error));
    }
    Ok(handle)
}

impl Handle {
    /// Releases the lock. Calling it more than once is harmless.
    ///
    /// The pid sidecar is removed **while the lock is still held**, not after:
    /// once the lock is gone the next process may already have taken it and
    /// written its own sidecar, and a removal ordered after the release could
    /// delete that fresh pid. A failed removal is ignored — it must never turn a
    /// successful release into an error.
    pub fn release(&mut self) -> Result<(), Error> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        let _ = fs::remove_file(&self.pid_path);
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        let unlocked = file.unlock();
        drop(file);
        unlocked.map_err(|error| io_error("unlock the lock file", &self.pid_path, &error))
    }

    /// Truncates the lock file and stamps the holder's pid.
    ///
    /// The lock is already held, so no other process can observe a partial write;
    /// the truncate also removes a stale pid left by a crashed predecessor.
    fn stamp_pid(&mut self) -> io::Result<()> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.set_len(0)?;
        // After truncating, the write position still belongs to a previous run.
        use std::io::Seek;
        file.seek(io::SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Writes the holder's pid into the sidecar.
fn write_pid_file(path: &Path) -> io::Result<()> {
    fs::write(path, format!("{}\n", std::process::id()))
}

/// Reads the holder's pid from the sidecar, or `0` when it is missing or does not
/// contain a usable pid.
fn read_pid(path: &Path) -> u64 {
    let Ok(file) = File::open(path) else {
        return 0;
    };
    let mut text = String::new();
    if file.take(64).read_to_string(&mut text).is_err() {
        return 0;
    }
    text.trim().parse().unwrap_or(0)
}

/// Wraps a filesystem failure as a kernel error, keeping `E_PERMISSION` for
/// access denials so callers can suggest a repair.
fn io_error(operation: &str, path: &Path, cause: &io::Error) -> Error {
    let code = if cause.kind() == io::ErrorKind::PermissionDenied {
        Code::Permission
    } else {
        Code::Internal
    };
    Error::new(code, format!("lock: cannot {operation} {path:?}: {cause}"))
        .with_param("error", cause.to_string())
        .with_param("path", path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use o824_fixtures::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new("o824-lock-").expect("temp dir")
    }

    #[test]
    fn the_lock_file_and_pid_sidecar_are_created() {
        let dir = temp_dir();
        let mut handle = acquire(dir.path()).expect("acquire");
        assert!(dir.path().join(LOCK_FILE_NAME).is_file());
        assert!(dir.path().join(PID_FILE_NAME).is_file());

        // The lock is mandatory: while it is held, no other handle may read the
        // file, so the stamp is checked after the release.
        handle.release().expect("release");
        let stamp = fs::read_to_string(dir.path().join(LOCK_FILE_NAME)).expect("read lock");
        assert_eq!(stamp.trim(), std::process::id().to_string());
    }

    #[test]
    fn releasing_twice_is_harmless() {
        let dir = temp_dir();
        let mut handle = acquire(dir.path()).expect("acquire");
        handle.release().expect("release");
        handle.release().expect("release again");
        assert!(!dir.path().join(PID_FILE_NAME).exists());
    }

    #[test]
    fn a_released_lock_can_be_taken_again() {
        let dir = temp_dir();
        let mut first = acquire(dir.path()).expect("first");
        first.release().expect("release");
        let _second = acquire(dir.path()).expect("second");
    }

    /// The contract is cross-process exclusion: a second process must see
    /// `E_BUSY` naming the holder's pid.
    #[test]
    fn a_second_process_cannot_take_the_lock() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let dir = temp_dir();
        let child = Command::new(std::env::current_exe().expect("current exe"))
            .args([
                "--exact",
                "tests::lock_holder_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("O824_LOCK_CHILD_DIR", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the holder");
        let child_pid = child.id();

        // Wait until the child says it holds the lock. The harness prints its
        // own banner first, so keep reading until the marker shows up.
        let mut child = child;
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut said_locked = false;
        for _ in 0..20 {
            let mut line = String::new();
            if stdout.read_line(&mut line).expect("read the child's line") == 0 {
                break;
            }
            if line.contains("locked") {
                said_locked = true;
                break;
            }
        }
        assert!(
            said_locked,
            "the holder never reported that it took the lock"
        );

        let error = acquire(dir.path()).expect_err("the second process must be refused");
        assert_eq!(error.code, Code::Busy);
        assert_eq!(
            error.params["pid"],
            serde_json_value(child_pid),
            "E_BUSY must name the process to wait for"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    /// Child half of [`a_second_process_cannot_take_the_lock`]: it takes the
    /// lock, says so, and holds it until the parent kills it.
    #[test]
    fn lock_holder_child() {
        let Ok(directory) = std::env::var("O824_LOCK_CHILD_DIR") else {
            // The parent did not ask for this test.
            return;
        };
        let _handle = acquire(Path::new(&directory)).expect("the child takes the lock");
        println!("locked");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    /// Renders a pid as the JSON number `E_BUSY` carries.
    fn serde_json_value(pid: u32) -> serde_json::Value {
        serde_json::Value::from(pid)
    }
}
