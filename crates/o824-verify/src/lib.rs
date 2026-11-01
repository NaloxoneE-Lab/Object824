//! The kernel's P0 diagnostic checks against one installation tree.
//!
//! Every check returns [`Finding`] values and no check writes to the
//! installation: `verify` only observes, `plan` decides what to repair. Each
//! check is a pure function of the tree on disk, so repeated runs over the same
//! tree produce identical findings.
//!
//! The division of labour between checks is deliberate and load-bearing:
//!
//! * a file that cannot be parsed is F16's fault, and the checks that read that
//!   file stay silent rather than reporting a second finding for one root
//!   cause;
//! * a missing file is not a fault at all — a profile without a patch file has
//!   no patch entries, an install without a workspace file allows no builds;
//! * F03 is advisory, not a fault, and never drives an automatic edit (see
//!   [`loader`] for why the criterion is unproven on 0.2.1-rc.3).
//!
//! This crate must never depend on `o824-plan`: a check does not know that a
//! repair exists.

#![forbid(unsafe_code)]

pub mod journal;
pub mod link;
pub mod loader;
pub mod pnpm;
pub mod profile;

use std::path::{Path, PathBuf};

use o824_model::{Envelope, Finding, Install};
use serde_json::{Map, Value};

/// The profile diagnosed when the installation lists none. `desktop` is the
/// only profile shape P0 knows, so an install whose profile list could not be
/// read is still checked instead of silently passing.
pub const DEFAULT_PROFILE: &str = "desktop";

/// The DSH home directory holding one directory per profile.
pub const PROFILES_DIR_NAME: &str = "profiles";

/// Marks a repair the plan builder can perform unattended.
pub const REPAIR_KIND_AUTO: &str = "auto";
/// Marks a repair the plan builder must never perform unattended.
pub const REPAIR_KIND_MANUAL: &str = "manual";

/// Runs every P0 check against one installation and returns them as one
/// versioned findings envelope.
///
/// The profile tree is read from `install.home` when it is set, because a DSH
/// home owns `profiles\` even when the install root lives elsewhere;
/// `install.root` is the fallback. Each listing in `install.profiles` — or
/// exactly [`DEFAULT_PROFILE`] when the list is empty — is checked by links,
/// bridges, loader entries, profile parsing, and the pnpm build gate, in that
/// order. `HalfInstall` is global: it reads the installation's journals once,
/// not once per profile.
///
/// Findings from different checks are concatenated and never deduplicated: two
/// profiles with the same fault are two findings, because they are two repairs.
pub fn doctor(install: &Install, data_root: &Path) -> Envelope {
    let root = if install.home.is_empty() {
        PathBuf::from(&install.root)
    } else {
        PathBuf::from(&install.home)
    };

    let profiles = if install.profiles.is_empty() {
        vec![DEFAULT_PROFILE.to_owned()]
    } else {
        install.profiles.clone()
    };

    let mut findings = Vec::new();
    for profile in &profiles {
        findings.extend(link::links(&root, profile));
        findings.extend(link::bridge(&root, profile));
        findings.extend(loader::loader_entries(&root, profile));
        findings.extend(profile::profile_parse(&root, profile));
        findings.extend(pnpm::pnpm_build_gate(&root, profile));
    }
    findings.extend(journal::half_install(data_root));

    sort_findings(&mut findings);
    Envelope::new(findings)
}

/// Orders findings by check id and then by their `path` parameter, so repeated
/// runs and the plan builder see identical output.
pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|left, right| {
        left.check_id
            .cmp(&right.check_id)
            .then_with(|| param_string(left, PARAM_PATH).cmp(param_string(right, PARAM_PATH)))
    });
}

/// Returns one finding parameter as a string, or `""` when it is missing or
/// holds another type. Sorting only needs a stable key.
pub fn param_string<'a>(finding: &'a Finding, key: &str) -> &'a str {
    finding
        .params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Builds a parameter map whose keys are in ascending order.
///
/// The reference implementation marshals a Go map in key order, so the order is
/// part of the byte-level golden contract; `serde_json` preserves insertion
/// order, which means the insertion order has to be the sorted one.
pub fn params(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut sorted = pairs.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    sorted
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// Renders `path` relative to `root` when it can, so parameters do not carry a
/// machine-specific absolute path needlessly.
pub fn rel_path(root: &Path, path: &Path) -> String {
    let path = strip_nt_namespace(path);
    match relative_to(root, &path) {
        Some(relative) if !relative.is_empty() => relative,
        _ => path.display().to_string(),
    }
}

/// Turns a path Windows reports in the object namespace back into the Win32
/// spelling, so it can be compared with the installation root.
///
/// `\??\Global\` is how a secondary volume's drive letter appears inside the
/// object namespace; the junction still resolves exactly like `\??\D:\`, but
/// leaving the prefix in place makes a finding's target an opaque absolute NT
/// path.
pub fn strip_nt_namespace(path: &Path) -> PathBuf {
    let mut value = path.display().to_string();
    for prefix in ["\\\\?\\", "\\??\\"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.to_owned();
        }
    }
    if let Some(rest) = value.strip_prefix("Global\\") {
        value = rest.to_owned();
    }
    PathBuf::from(value)
}

/// Computes `path` relative to `root` the way the reference implementation's
/// `filepath.Rel` does.
///
/// The result may climb out of `root`: a link whose target lives beside the DSH
/// home is reported as `..\vendor\x`, not as an absolute path, because both
/// spellings name the same directory and only one of them is machine-independent.
/// `None` means the two paths share no volume and cannot be relative to each
/// other.
fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let root_parts: Vec<(String, String)> = components(root);
    let path_parts: Vec<(String, String)> = components(path);
    if root_parts.is_empty() || path_parts.is_empty() {
        return None;
    }

    let mut common = 0;
    while common < root_parts.len()
        && common < path_parts.len()
        && root_parts[common].0 == path_parts[common].0
    {
        common += 1;
    }
    // Different volumes share at most a separator, never a prefix.
    if common == 0 {
        return None;
    }

    let separator = std::path::MAIN_SEPARATOR.to_string();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..root_parts.len() {
        parts.push("..".to_owned());
    }
    for (_, original) in &path_parts[common..] {
        parts.push(original.clone());
    }
    if parts.is_empty() {
        return Some(".".to_owned());
    }
    Some(parts.join(&separator))
}

/// The components of a path as `(lowercase, original)` pairs.
fn components(path: &Path) -> Vec<(String, String)> {
    path.components()
        .map(|component| {
            let original = component.as_os_str().to_string_lossy().into_owned();
            let lowered = original.to_lowercase();
            (lowered, original)
        })
        .collect()
}

/// One finding parameter key: the path a finding is about.
pub const PARAM_PATH: &str = "path";
/// One finding parameter key: a link's target.
pub const PARAM_TARGET: &str = "target";
/// One finding parameter key: a plugin package name.
pub const PARAM_PLUGIN: &str = "plugin";
/// One finding parameter key: a package name.
pub const PARAM_PACKAGE: &str = "package";
/// One finding parameter key: a profile file, relative to the root.
pub const PARAM_FILE: &str = "file";
/// One finding parameter key: a parser's line number.
pub const PARAM_LINE: &str = "line";
/// One finding parameter key: the shape rule a file breaks.
pub const PARAM_REASON: &str = "reason";
/// One finding parameter key: the size of a colliding id set.
pub const PARAM_DUPLICATES: &str = "duplicates";
/// One finding parameter key: the colliding ids.
pub const PARAM_IDS: &str = "ids";
/// One finding parameter key: the `allowBuilds` key a repair has to write.
pub const PARAM_KEY: &str = "key";
/// One finding parameter key: a journal file name.
pub const PARAM_JOURNAL: &str = "journal";
/// One finding parameter key: the state a journal recorded.
pub const PARAM_STATE: &str = "state";

#[cfg(test)]
mod tests {
    use super::*;
    use o824_model::{Category, Code, Severity};

    #[test]
    fn params_are_stored_in_sorted_key_order() {
        let built = params(&[
            ("path", Value::from("x")),
            ("target", Value::from("y")),
            ("key", Value::from("z")),
        ]);
        let keys: Vec<&str> = built.keys().map(String::as_str).collect();
        assert_eq!(keys, ["key", "path", "target"]);
    }

    #[test]
    fn findings_sort_by_check_id_then_path() {
        let mut findings = vec![
            Finding::new(
                "F04",
                Severity::Error,
                Code::BridgeDangling,
                Category::Plugin,
            )
            .with_params(params(&[("path", Value::from("b"))])),
            Finding::new(
                "F04",
                Severity::Error,
                Code::BridgeDangling,
                Category::Plugin,
            )
            .with_params(params(&[("path", Value::from("a"))])),
            Finding::new("F01", Severity::Error, Code::LinkBroken, Category::Plugin)
                .with_params(params(&[("path", Value::from("z"))])),
        ];
        sort_findings(&mut findings);
        let order: Vec<(&str, &str)> = findings
            .iter()
            .map(|finding| (finding.check_id.as_str(), param_string(finding, PARAM_PATH)))
            .collect();
        assert_eq!(order, [("F01", "z"), ("F04", "a"), ("F04", "b")]);
    }

    #[test]
    fn relative_paths_use_the_root_as_prefix() {
        let root = Path::new("C:\\tmp\\root");
        assert_eq!(
            rel_path(root, Path::new("C:\\tmp\\root\\profiles\\desktop")),
            "profiles\\desktop"
        );
        // Case-insensitive on Windows.
        assert_eq!(rel_path(root, Path::new("c:\\TMP\\root\\a")), "a");
        // A sibling beside the root is reported by climbing out, not absolutely:
        // that is what the reference implementation's `filepath.Rel` does, and
        // only one of the two spellings is machine-independent.
        assert_eq!(
            rel_path(root, Path::new("C:\\tmp\\rooty\\a")),
            "..\\rooty\\a"
        );
    }

    #[test]
    fn nt_namespace_prefixes_are_stripped() {
        assert_eq!(
            strip_nt_namespace(Path::new("\\\\?\\D:\\x")),
            PathBuf::from("D:\\x")
        );
        assert_eq!(
            strip_nt_namespace(Path::new("\\??\\Global\\D:\\x")),
            PathBuf::from("D:\\x")
        );
    }

    #[test]
    fn the_doctor_on_the_ok_tree_is_clean() {
        let fixture = o824_fixtures::build("ok").expect("build ok");
        let install = o824_discovery_install(fixture.root());
        let envelope = doctor(&install, fixture.root());
        assert_eq!(envelope.findings, Vec::new());
        assert_eq!(envelope.envelope, "dsh-doctor/v1");
    }

    /// The fixture tree doubles as its own DSH home, which is what the golden
    /// harness relies on.
    fn o824_discovery_install(root: &Path) -> Install {
        Install {
            install_id: "i1".to_owned(),
            kind: "desktop".to_owned(),
            root: root.display().to_string(),
            version: o824_fixtures::DSH_VERSION.to_owned(),
            profiles: vec!["desktop".to_owned()],
            health: "ok".to_owned(),
            home: root.display().to_string(),
        }
    }
}
