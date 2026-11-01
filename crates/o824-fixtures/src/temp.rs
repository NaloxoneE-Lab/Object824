//! A temporary directory that deletes itself.
//!
//! The fixtures and their consumers need one thing from a temp directory: a
//! unique path that disappears when the test ends. `tempfile` would do that, but
//! its current release pulls in `getrandom`, and `getrandom`'s Windows support
//! links with `raw-dylib` — which makes rustc look for `dlltool` on a
//! `windows-gnu` host. This machine has no MinGW toolchain (the local linker is
//! the Rust toolchain's own `rust-lld`), so that dependency is a build failure
//! for a feature the fixtures never use.
//!
//! The name is `{prefix}{pid}-{sequence}-{nanos}`: unique per process, per call,
//! and per run. It is a test helper, not a security boundary.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A temporary directory that removes itself on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a new temporary directory whose name starts with `prefix`.
    pub fn new(prefix: &str) -> io::Result<TempDir> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);

        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let name = format!("{prefix}{}-{sequence}-{nanos}", std::process::id());

        let path = std::env::temp_dir().join(name);
        fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // A leftover directory is a nuisance, never a test failure: the process
        // may be unwinding, or a file may still be open on Windows.
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_dir_is_created_and_removed() {
        let path = {
            let dir = TempDir::new("o824-test-").expect("create");
            assert!(dir.path().is_dir());
            dir.path().to_path_buf()
        };
        assert!(!path.exists(), "the directory must be gone after drop");
    }

    #[test]
    fn two_temp_dirs_never_collide() {
        let first = TempDir::new("o824-test-").expect("create");
        let second = TempDir::new("o824-test-").expect("create");
        assert_ne!(first.path(), second.path());
    }
}
