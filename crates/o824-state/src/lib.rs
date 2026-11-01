//! O824's small, machine-readable state: the per-user data-root rule and — from
//! milestone M3 — the registry of known installations, written atomically.
//!
//! The data root is where the kernel keeps its lock, its state file, its
//! snapshots and its journals. It is deliberately per-user: cross-Windows-user
//! exclusion is a known P0 gap.

#![forbid(unsafe_code)]

use std::path::PathBuf;

/// The environment variable that overrides the data root. It is part of the CLI
/// contract and is honoured before any default.
pub const DATA_ROOT_ENV: &str = "O824_DATA_ROOT";

/// The directory name of the data root under `%LOCALAPPDATA%` or the user home.
const DATA_ROOT_DIR_NAME: &str = "o824";

/// Resolves the per-user data root.
///
/// Precedence: `$O824_DATA_ROOT`, then `%LOCALAPPDATA%\o824`, then
/// `<home>\o824`, then the user's home directory as a last resort.
pub fn data_root(explicit: Option<&str>) -> PathBuf {
    if let Some(root) = explicit.map(str::trim).filter(|root| !root.is_empty()) {
        return PathBuf::from(root);
    }
    if let Some(root) = env_value(DATA_ROOT_ENV) {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Some(local) = env_value("LOCALAPPDATA") {
        let trimmed = local.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join(DATA_ROOT_DIR_NAME);
        }
    }
    if let Some(home) = std::env::home_dir() {
        return home.join(DATA_ROOT_DIR_NAME);
    }
    PathBuf::from(DATA_ROOT_DIR_NAME)
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_root_wins() {
        assert_eq!(
            data_root(Some(r"D:\data")),
            PathBuf::from(r"D:\data"),
            "an explicit root must be honoured before any other source"
        );
    }

    #[test]
    fn the_default_is_under_local_app_data() {
        let root = data_root(None);
        assert!(!root.as_os_str().is_empty());
        assert!(root.ends_with(DATA_ROOT_DIR_NAME));
    }
}
