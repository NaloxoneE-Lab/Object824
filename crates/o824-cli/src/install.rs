//! Installation selection, shared by `discover`, `doctor` and `repair`.
//!
//! All three commands agree on what `--root`, `--home` and `--install` mean, so
//! the rules live in one place: an explicit root is *inspected*, never created,
//! and a named install id goes through the machine scan.

use std::path::Path;

use o824_errs::{Code, Error};
use o824_model::Install;

/// Resolves the installation a command is about.
///
/// `--root` bypasses discovery's machine scan: the path is taken as given, and it
/// doubles as the DSH home unless `--home` overrides that. Otherwise `--install`
/// selects one discovered installation by id; with neither, the command refuses
/// to guess, because diagnosing or repairing the wrong installation is worse than
/// doing nothing.
pub fn select(
    command: &str,
    install_id: Option<&str>,
    root: Option<&str>,
    home: Option<&str>,
) -> Result<Install, Error> {
    if let Some(root) = root.filter(|root| !root.is_empty()) {
        // A typo'd --root must fail instead of diagnosing nothing and exiting 0
        // with "no findings". The path is inspected, never created.
        require_root_dir(root)?;
        // A caller may spell the root with `.` or `..` components (the lab does,
        // because it reaches the workspace through its parent). The reference
        // implementation cleans every path it joins, so the findings it reports
        // never carry those components; cleaning here keeps the two
        // implementations' idea of "where the tree is" identical.
        let root = o824_model::path::clean(Path::new(root))
            .display()
            .to_string();
        let home = home.filter(|home| !home.is_empty()).unwrap_or(&root);
        let home = o824_model::path::clean(Path::new(home))
            .display()
            .to_string();
        // Profiles are enumerated from the home the same way discovery does, so a
        // tree whose only profile is web, tui, or headless is diagnosed too.
        let profiles = o824_discovery::profile_names(Path::new(&home));
        let install_id = match install_id.filter(|id| !id.is_empty()) {
            Some(id) => id.to_owned(),
            // A root names an installation even without an --install label, and
            // the plan builder refuses an installation with no id.
            None => local_install_id(&root),
        };
        // The shape is read from the tree, not assumed: with three forms, `--root` on a
        // tui package that was diagnosed as an installation would skip the tui's own
        // checks and report a clean bill of health for a tree it never understood. Trees
        // that do not validate (the lab's broken ones) keep the old answer, because
        // "damaged desktop" is still the useful description for them.
        let kind = o824_discovery::validate_with(Path::new(&root), Path::new(&home), None)
            .map(|install| install.kind)
            .unwrap_or_else(|_| o824_discovery::KIND_DESKTOP.to_owned());
        return Ok(Install {
            install_id,
            kind,
            root: root.clone(),
            version: String::new(),
            profiles,
            health: String::new(),
            home,
        });
    }

    if let Some(install_id) = install_id.filter(|id| !id.is_empty()) {
        let installs = o824_discovery::discover(&o824_discovery::Options {
            roots: None,
            extra_dirs: Vec::new(),
            home: home.map(str::to_owned),
            ..Default::default()
        })?;
        return installs
            .into_iter()
            .find(|install| install.install_id == install_id)
            .ok_or_else(|| {
                Error::new(
                    Code::Usage,
                    format!("o824 {command}: no installation with id {install_id:?}"),
                )
            });
    }

    Err(Error::new(
        Code::Usage,
        format!("o824 {command}: pass --install <id> or --root <path>"),
    ))
}

/// Checks that `root` names a directory.
pub fn require_root_dir(root: &str) -> Result<(), Error> {
    let clean = o824_model::path::clean(Path::new(root))
        .display()
        .to_string();
    let is_dir = std::fs::metadata(&clean)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    if is_dir {
        return Ok(());
    }
    Err(Error::new(
        Code::InstallDir,
        format!("{clean:?} is not an installation directory"),
    )
    .with_param("root", clean))
}

/// Derives the stable store key for a root the user named directly: it is
/// `local-<12 hex of sha256(lowercased realpath)>`.
pub fn local_install_id(root: &str) -> String {
    use sha2::{Digest, Sha256};

    let cleaned = o824_model::path::clean(Path::new(root))
        .display()
        .to_string();
    let resolved = std::fs::canonicalize(&cleaned)
        .map(|path| {
            o824_model::path::strip_nt_namespace(&path)
                .display()
                .to_string()
        })
        .unwrap_or(cleaned);
    let mut hasher = Sha256::new();
    hasher.update(resolved.to_lowercase().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(12);
    for byte in &digest[..6] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("local-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_root_is_e_install_dir() {
        let error = require_root_dir(r"C:\definitely\not\here").expect_err("must fail");
        assert_eq!(error.code, Code::InstallDir);
        assert!(error.params.contains_key("root"));
    }

    #[test]
    fn neither_flag_is_a_usage_error() {
        let error = select("repair", None, None, None).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(error
            .fallback
            .contains("pass --install <id> or --root <path>"));
    }

    #[test]
    fn an_explicit_root_and_an_id_combine() {
        let directory = o824_fixtures::TempDir::new("o824-select-").expect("temp dir");
        let root = directory.path().display().to_string();
        let install = select("doctor", Some("labelled"), Some(&root), None).expect("select");
        assert_eq!(install.install_id, "labelled");
        assert_eq!(install.root, root);
        assert_eq!(install.home, root);
    }

    #[test]
    fn a_root_without_an_id_gets_a_stable_local_id() {
        let directory = o824_fixtures::TempDir::new("o824-select-").expect("temp dir");
        let root = directory.path().display().to_string();
        let install = select("doctor", None, Some(&root), None).expect("select");
        assert!(
            install.install_id.starts_with("local-"),
            "{}",
            install.install_id
        );
        assert_eq!(install.install_id.len(), "local-".len() + 12);
    }

    #[test]
    fn an_unknown_install_id_is_a_usage_error() {
        let error =
            select("repair", Some("i-definitely-not-there"), None, None).expect_err("must fail");
        assert_eq!(error.code, Code::Usage);
        assert!(
            error.fallback.contains("no installation with id"),
            "{error}"
        );
    }
}
