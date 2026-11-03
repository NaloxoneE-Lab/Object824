// The web form: `npm i -g @deepseek-ai/dsh` puts one package directory under a global
// prefix, and that is the whole installation.
//
// The prefix is computed rather than asked for. Running `npm prefix -g` on every
// discovery would cost a few hundred milliseconds and depend on npm being on PATH; the
// rule npm uses on Windows is `%APPDATA%\npm`, and pnpm's is the `global\5` directory
// under `%LOCALAPPDATA%\pnpm` (or whatever `PNPM_HOME` says).

use std::path::PathBuf;

use crate::machine::Source;
use crate::{PACKAGE_JSON_NAME, WEB_PACKAGE_NAME};

/// The global prefixes this machine may have installed a package into.
///
/// Ordered most likely first. A prefix that does not exist contributes nothing.
pub fn global_prefixes() -> Vec<PathBuf> {
    let mut prefixes = Vec::new();
    if let Some(home) = env_path("PNPM_HOME") {
        prefixes.push(home.clone());
        prefixes.push(home.join("global").join("5"));
    }
    if let Some(appdata) = env_path("APPDATA") {
        prefixes.push(appdata.join("npm"));
    }
    if let Some(local) = env_path("LOCALAPPDATA") {
        prefixes.push(local.join("pnpm").join("global").join("5"));
        prefixes.push(local.join("pnpm"));
    }
    prefixes
}

/// One environment variable as an absolute path, when it is set and not blank.
fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// The `node_modules/@deepseek-ai/dsh` directory of one global prefix, when it exists.
pub fn package_in_prefix(prefix: &std::path::Path) -> Option<PathBuf> {
    let mut path = prefix.join("node_modules");
    for part in WEB_PACKAGE_NAME.split('/') {
        path = path.join(part);
    }
    // A directory is not enough: a half-removed package leaves the directory behind.
    path.join(PACKAGE_JSON_NAME).is_file().then_some(path)
}

/// The tui package inside `prefix`, when it is installed there.
pub fn tui_package_in_prefix(prefix: &std::path::Path) -> Option<PathBuf> {
    let mut path = prefix.join("node_modules");
    for part in crate::TUI_PACKAGE_NAME.split('/') {
        path = path.join(part);
    }
    path.join(PACKAGE_JSON_NAME).is_file().then_some(path)
}

/// The tui installations among `prefixes`.
pub fn tui_roots(prefixes: &[PathBuf]) -> Vec<crate::machine::SourceRoot> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut roots = Vec::new();
    for prefix in prefixes {
        let Some(package) = tui_package_in_prefix(prefix) else {
            continue;
        };
        if seen.iter().any(|other| other == &package) {
            continue;
        }
        seen.push(package.clone());
        roots.push(crate::machine::SourceRoot {
            root: package.display().to_string(),
            registry_version: None,
            source: Source::Web,
        });
    }
    roots
}

/// The web installations among `prefixes`.
pub fn web_roots(prefixes: &[PathBuf]) -> Vec<crate::machine::SourceRoot> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut roots = Vec::new();
    for prefix in prefixes {
        let Some(package) = package_in_prefix(prefix) else {
            continue;
        };
        if seen.iter().any(|other| other == &package) {
            continue;
        }
        seen.push(package.clone());
        roots.push(crate::machine::SourceRoot {
            root: package.display().to_string(),
            registry_version: None,
            source: Source::Web,
        });
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(prefix: &std::path::Path, version: &str) {
        let mut root = prefix.join("node_modules");
        for part in WEB_PACKAGE_NAME.split('/') {
            root = root.join(part);
        }
        std::fs::create_dir_all(root.join("lib")).expect("create lib");
        std::fs::write(
            root.join(PACKAGE_JSON_NAME),
            format!("{{\"name\":\"{WEB_PACKAGE_NAME}\",\"version\":\"{version}\",\"bin\":{{\"dsh\":\"lib/bin.js\"}}}}"),
        )
        .expect("write manifest");
        std::fs::write(root.join("lib").join("bin.js"), b"// entry").expect("write entry");
    }

    #[test]
    fn a_global_prefix_with_the_package_is_a_web_root() {
        let temp = o824_fixtures::TempDir::new("o824-web-prefix-").expect("temp dir");
        write_package(temp.path(), "0.2.1-rc.5");

        let roots = web_roots(&[temp.path().to_path_buf()]);
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(roots[0].source, Source::Web);
        assert!(roots[0].root.ends_with("dsh"), "{}", roots[0].root);
    }

    #[test]
    fn a_prefix_without_the_package_contributes_nothing() {
        let temp = o824_fixtures::TempDir::new("o824-web-prefix-").expect("temp dir");
        std::fs::create_dir_all(temp.path().join("node_modules")).expect("mkdir");
        assert!(web_roots(&[temp.path().to_path_buf()]).is_empty());
        assert!(web_roots(&[temp.path().join("missing")]).is_empty());
    }

    #[test]
    fn a_half_removed_package_does_not_count() {
        let temp = o824_fixtures::TempDir::new("o824-web-prefix-").expect("temp dir");
        let mut root = temp.path().join("node_modules");
        for part in WEB_PACKAGE_NAME.split('/') {
            root = root.join(part);
        }
        std::fs::create_dir_all(&root).expect("create the directory but no manifest");
        assert!(web_roots(&[temp.path().to_path_buf()]).is_empty());
    }
}
