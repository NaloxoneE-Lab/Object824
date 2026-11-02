// The bounded scan: the last resort, for an installation the user put somewhere no
// rule predicts.
//
// It is deliberately small and cheap. Discovery runs on every `o824` command, so a
// scan that walks a whole drive would make the tool feel broken; a fixed time budget
// and an entry cap bound it, and a scan that runs out of budget returns what it found
// so far rather than nothing. The only place it looks is a short list of directories
// where people install programs: the depth limit is what keeps "Program Files" from
// turning into "the whole volume".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::machine::{Source, SourceRoot};
use crate::{ASAR_REL_PATH, EXE_NAME};

/// How deep below a scan root the walk goes.
const MAX_DEPTH: usize = 3;
/// The default time budget; a scan that has not finished by then stops.
pub const DEFAULT_BUDGET: Duration = Duration::from_millis(400);
/// The default cap on directories visited.
pub const DEFAULT_MAX_ENTRIES: usize = 5000;

/// The directories the bounded scan walks by default.
///
/// `Program Files` on every fixed volume, because a machine with DSH on `D:` does not
/// necessarily have it on `C:`.
pub fn default_scan_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for volume in crate::machine::fixed_volumes() {
        dirs.push(PathBuf::from(&volume).join("Program Files"));
        dirs.push(PathBuf::from(&volume).join("Program Files (x86)"));
    }
    if let Some(local) = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        dirs.push(PathBuf::from(local).join("Programs"));
    }
    dirs
}

/// The installation roots found under `dirs` within the budget.
pub fn scan_roots(dirs: &[PathBuf], budget: Duration, max_entries: usize) -> Vec<SourceRoot> {
    let deadline = Instant::now() + budget;
    let mut roots = Vec::new();
    let mut visited = 0usize;

    let mut stack: Vec<(PathBuf, usize)> = dirs.iter().map(|dir| (dir.clone(), 0)).collect();
    while let Some((directory, depth)) = stack.pop() {
        if visited >= max_entries || Instant::now() >= deadline {
            // Out of budget: keep what was found. A partial answer is still an answer,
            // and the layers above it already ran.
            break;
        }
        visited += 1;

        if looks_like_install(&directory) {
            roots.push(SourceRoot {
                root: directory.display().to_string(),
                registry_version: None,
                source: Source::Scan,
            });
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .map(|entry| entry.path())
            .collect();
        // Sorted, then pushed in reverse, so the walk is deterministic.
        children.sort();
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1));
        }
    }
    roots
}

/// True when `directory` holds the two files that make a tree an installation.
fn looks_like_install(directory: &Path) -> bool {
    directory.join(EXE_NAME).is_file() && directory.join(ASAR_REL_PATH).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_install(root: &Path) {
        std::fs::create_dir_all(root.join("resources")).expect("create resources");
        std::fs::write(root.join(EXE_NAME), b"stub").expect("write the exe");
        std::fs::write(root.join(ASAR_REL_PATH), b"stub").expect("write the asar");
    }

    #[test]
    fn a_custom_install_directory_is_found() {
        let temp = o824_fixtures::TempDir::new("o824-discovery-scan-").expect("temp dir");
        let root = temp.path().join("Vendor").join("DeepSeek Harness");
        fake_install(&root);

        let roots = scan_roots(
            &[temp.path().to_path_buf()],
            DEFAULT_BUDGET,
            DEFAULT_MAX_ENTRIES,
        );
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(roots[0].root, root.display().to_string());
        assert_eq!(roots[0].source, Source::Scan);
    }

    #[test]
    fn a_directory_that_only_half_looks_like_an_install_is_not_reported() {
        let temp = o824_fixtures::TempDir::new("o824-discovery-scan-").expect("temp dir");
        let half = temp.path().join("Half");
        std::fs::create_dir_all(&half).expect("create");
        std::fs::write(half.join(EXE_NAME), b"stub").expect("write the exe only");

        assert!(scan_roots(
            &[temp.path().to_path_buf()],
            DEFAULT_BUDGET,
            DEFAULT_MAX_ENTRIES
        )
        .is_empty());
    }

    #[test]
    fn the_walk_stops_at_the_entry_budget() {
        let temp = o824_fixtures::TempDir::new("o824-discovery-scan-").expect("temp dir");
        for index in 0..50 {
            std::fs::create_dir_all(temp.path().join(format!("dir{index:02}"))).expect("create");
        }
        // An install below the budget: it must not be reached, because the walk stops
        // as soon as the cap is hit.
        fake_install(&temp.path().join("dir49").join("DeepSeek Harness"));

        let roots = scan_roots(&[temp.path().to_path_buf()], DEFAULT_BUDGET, 5);
        assert!(roots.is_empty(), "the budget must stop the walk: {roots:?}");
    }

    #[test]
    fn the_depth_limit_keeps_the_walk_shallow() {
        let temp = o824_fixtures::TempDir::new("o824-discovery-scan-").expect("temp dir");
        let deep = temp
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("d")
            .join("DeepSeek Harness");
        fake_install(&deep);

        // Five levels down is past MAX_DEPTH, so it is not found — the bound that keeps
        // the scan from turning into a drive walk.
        assert!(scan_roots(
            &[temp.path().to_path_buf()],
            DEFAULT_BUDGET,
            DEFAULT_MAX_ENTRIES
        )
        .is_empty());
    }
}
