//! The cross-implementation acceptance pipeline.
//!
//! `spec/golden` and `spec/transcripts` are a contract between implementations,
//! not a description of one: the same fixture tree has to produce the same JSON
//! from Go and from Rust. This crate turns that sentence into executable
//! assertions:
//!
//! * [`load`] reads a contract file as text, so a comparison is byte-for-byte.
//! * [`normalize_text`] replaces exactly the values that cannot be
//!   machine-independent — absolute paths, plan ids, timestamps, the derived
//!   install id — and nothing else.
//! * [`compare`] renders a unified diff instead of the word "mismatch", because
//!   a failing golden must say which byte differs.
//! * [`read_manifests`] pins the crate graph so a forbidden dependency cannot be
//!   added by accident.
//!
//! At milestone M0 the "implementation" is a stub that replays the golden files;
//! the tests that use these helpers therefore prove the pipeline itself, which
//! is exactly what has to be trustworthy before `verify` exists.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The eight fixture kinds every golden file is named after.
pub const KINDS: [&str; 8] = [
    "ok",
    "broken-link",
    "missing-bundle",
    "dup-loader",
    "parse-error",
    "half-install",
    "dangling-bridge",
    "git-dep",
];

/// The repository root (`o824-rs`).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

/// The contract directory (`o824-rs/spec`).
pub fn spec_dir() -> PathBuf {
    repo_root().join("spec")
}

/// The golden directory (`o824-rs/spec/golden`).
pub fn golden_dir() -> PathBuf {
    spec_dir().join("golden")
}

/// The transcript directory (`o824-rs/spec/transcripts`).
pub fn transcript_dir() -> PathBuf {
    spec_dir().join("transcripts")
}

/// The golden file for `kind` and `document` (`doctor` or `plan`).
pub fn golden_file(kind: &str, document: &str) -> PathBuf {
    golden_dir().join(format!("{kind}.{document}.json"))
}

/// Reads a UTF-8 text file, panicking with the path when it cannot be read.
pub fn load(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The values a comparison has to treat as machine-dependent.
///
/// Everything not listed here — `seq`, `params`, `repair`, `fingerprint`,
/// `fallbackMessage` — is part of the contract and must match byte for byte.
#[derive(Clone, Debug, Default)]
pub struct NormalizeCtx {
    /// The fixture root, which is also its DSH home.
    pub root: PathBuf,
    /// The data root, when a tree's journal was copied out of the fixture.
    pub data_root: Option<PathBuf>,
    /// The plan id.
    pub plan_id: String,
    /// The derived install id.
    pub install_id: String,
    /// The plan's creation time, RFC 3339.
    pub created_at: String,
    /// The plan's expiry time, RFC 3339.
    pub expires_at: String,
}

impl NormalizeCtx {
    /// A context that only knows the fixture root, which is all a `doctor`
    /// document needs.
    pub fn for_root(root: impl Into<PathBuf>) -> NormalizeCtx {
        NormalizeCtx {
            root: root.into(),
            ..NormalizeCtx::default()
        }
    }
}

/// Replaces exactly the machine-dependent values in a JSON document.
///
/// Replacements are textual and ordered longest-first, so a shorter needle can
/// never eat part of a longer one. Paths are handled in three spellings: the
/// native form (`D:\x`), the JSON-escaped form (`D:\\x`), and the forward-slash
/// form — Windows remembers paths both ways and a golden that only matched one
/// of them would fail on half the machines.
pub fn normalize_text(text: &str, ctx: &NormalizeCtx) -> String {
    let mut pairs: Vec<(String, &'static str)> = Vec::new();

    let mut add_path = |path: &Path, placeholder: &'static str| {
        let native = path.display().to_string();
        if native.is_empty() {
            return;
        }
        pairs.push((native.replace('\\', "\\\\"), placeholder));
        pairs.push((native.replace('\\', "/"), placeholder));
        pairs.push((native, placeholder));
    };

    add_path(&ctx.root, "<ROOT>");
    if let Some(data_root) = &ctx.data_root {
        add_path(data_root, "<DATA>");
    }
    for (value, placeholder) in [
        (&ctx.plan_id, "<PLAN_ID>"),
        (&ctx.install_id, "<INSTALL_ID>"),
        (&ctx.created_at, "<TIMESTAMP>"),
        (&ctx.expires_at, "<TIMESTAMP>"),
    ] {
        if !value.is_empty() {
            pairs.push((value.clone(), placeholder));
        }
    }

    pairs.sort_by_key(|pair| std::cmp::Reverse(pair.0.len()));

    let mut normalized = text.to_owned();
    for (needle, placeholder) in pairs {
        if normalized.contains(&needle) {
            normalized = normalized.replace(&needle, placeholder);
        }
    }
    normalized
}

/// Compares an expected document with an actual one.
///
/// Returns `Ok(())` when they are identical, and a unified diff otherwise. The
/// diff is the point: "mismatch" on its own costs an hour per regression.
pub fn compare(expected: &str, actual: &str, label: &str) -> Result<(), String> {
    if expected == actual {
        return Ok(());
    }
    Err(diff_report(expected, actual, label))
}

/// Asserts that an actual document matches its expected contract.
///
/// # Panics
///
/// Panics with a unified diff when the documents differ.
pub fn assert_golden(expected: &str, actual: &str, label: &str) {
    if let Err(report) = compare(expected, actual, label) {
        panic!("{report}");
    }
}

/// Renders a unified diff between the expected and the actual document.
pub fn diff_report(expected: &str, actual: &str, label: &str) -> String {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(expected, actual);
    let mut report = String::new();
    report.push_str(&format!("golden mismatch: {label}\n"));
    report.push_str("--- expected\n+++ actual\n");

    for group in diff.grouped_ops(3) {
        let mut old_start = None;
        let mut new_start = None;
        let mut old_len = 0;
        let mut new_len = 0;
        for op in &group {
            let old = op.old_range();
            let new = op.new_range();
            old_start.get_or_insert(old.start);
            new_start.get_or_insert(new.start);
            old_len += old.len();
            new_len += new.len();
        }
        report.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start.unwrap_or(0) + 1,
            old_len,
            new_start.unwrap_or(0) + 1,
            new_len
        ));
        for op in &group {
            for change in diff.iter_changes(op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                report.push(sign);
                report.push_str(change.value());
                if !change.value().ends_with('\n') {
                    report.push('\n');
                }
            }
        }
    }
    report
}

/// One crate's dependency view, as read from its `Cargo.toml`.
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    /// The package name.
    pub name: String,
    /// Normal dependencies.
    pub dependencies: Vec<String>,
    /// Development dependencies.
    pub dev_dependencies: Vec<String>,
    /// Build dependencies.
    pub build_dependencies: Vec<String>,
}

impl Manifest {
    /// Every dependency name, whatever the section.
    pub fn all_dependencies(&self) -> impl Iterator<Item = &String> {
        self.dependencies
            .iter()
            .chain(self.dev_dependencies.iter())
            .chain(self.build_dependencies.iter())
    }
}

/// Reads every crate manifest under `crates/`, keyed by package name.
pub fn read_manifests() -> BTreeMap<String, Manifest> {
    let crates_dir = repo_root().join("crates");
    let mut manifests = BTreeMap::new();
    let entries = std::fs::read_dir(&crates_dir).unwrap_or_else(|error| {
        panic!("cannot read {}: {error}", crates_dir.display());
    });
    for entry in entries.flatten() {
        let manifest_path = entry.path().join("Cargo.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest = parse_manifest(&load(&manifest_path));
        if !manifest.name.is_empty() {
            manifests.insert(manifest.name.clone(), manifest);
        }
    }
    manifests
}

/// Parses the parts of a `Cargo.toml` the crate-graph rules need.
///
/// This is deliberately not a TOML parser: the manifests in this workspace are
/// flat `name = …` tables, and a smaller reader is easier to trust than a
/// dependency that would itself have to be audited.
pub fn parse_manifest(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let mut section = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).trim().to_owned();
            continue;
        }
        let Some(name) = dependency_key(line) else {
            continue;
        };
        match section.as_str() {
            "package" if name == "name" => {
                manifest.name = value_of(line).unwrap_or_default();
            }
            "dependencies" => manifest.dependencies.push(name),
            "dev-dependencies" => manifest.dev_dependencies.push(name),
            "build-dependencies" => manifest.build_dependencies.push(name),
            _ => {}
        }
    }
    manifest
}

/// Returns the key of a `key = value` or `key.workspace = true` line.
fn dependency_key(line: &str) -> Option<String> {
    let key: String = line
        .chars()
        .take_while(|ch| *ch != '=' && *ch != '.')
        .collect();
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_owned())
    }
}

/// Returns the quoted value of a `key = "value"` line.
fn value_of(line: &str) -> Option<String> {
    let (_, value) = line.split_once('=')?;
    Some(value.trim().trim_matches('"').to_owned())
}

/// A dependency edge the architecture forbids, with the reason it is forbidden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForbiddenEdge {
    /// The crate holding the dependency.
    pub crate_name: String,
    /// The dependency it must not hold.
    pub dependency: String,
    /// Why the edge is forbidden.
    pub reason: &'static str,
}

/// Every edge in the crate graph that the architecture forbids.
///
/// The rules come from `docs/week1/01-workspace.md`: a check does not know that
/// a repair exists, a plan does not know that execution exists, the kernel does
/// not know that human-facing output exists, and the two leaf crates do not know
/// about the kernel at all. A cycle would be caught by cargo; these edges are
/// legal to cargo and fatal to the design.
pub fn forbidden_edges(manifests: &BTreeMap<String, Manifest>) -> Vec<ForbiddenEdge> {
    const RULES: [(&str, &str, &str); 4] = [
        (
            "o824-verify",
            "o824-plan",
            "a check must not know that a repair exists",
        ),
        (
            "o824-plan",
            "o824-apply",
            "a plan must not know that execution exists",
        ),
        (
            "o824-apply",
            "o824-cli",
            "the kernel must not know that human-facing output exists",
        ),
        (
            "o824-model",
            "o824-plan",
            "the shared model must stay below every other crate",
        ),
    ];

    let mut violations = Vec::new();
    for (crate_name, dependency, reason) in RULES {
        if let Some(manifest) = manifests.get(crate_name) {
            if manifest.all_dependencies().any(|name| name == dependency) {
                violations.push(ForbiddenEdge {
                    crate_name: crate_name.to_owned(),
                    dependency: dependency.to_owned(),
                    reason,
                });
            }
        }
    }

    // The leaves stay at the bottom: `o824-errs` depends on nothing internal,
    // and `o824-model` may only sit on top of it. The model edge exists because
    // a finding carries an error code — the reference implementation has the
    // same edge (`model` imports `internal/errs`), so "neither leaf depends on
    // any internal crate" would be a rule the design cannot satisfy.
    if let Some(manifest) = manifests.get("o824-errs") {
        for dependency in manifest.all_dependencies() {
            if dependency.starts_with("o824-") {
                violations.push(ForbiddenEdge {
                    crate_name: "o824-errs".to_owned(),
                    dependency: dependency.clone(),
                    reason: "the error codes are the floor: no internal dependencies",
                });
            }
        }
    }
    if let Some(manifest) = manifests.get("o824-model") {
        for dependency in manifest.all_dependencies() {
            if dependency.starts_with("o824-") && dependency != "o824-errs" {
                violations.push(ForbiddenEdge {
                    crate_name: "o824-model".to_owned(),
                    dependency: dependency.clone(),
                    reason: "the shared model may only sit on the error codes",
                });
            }
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_replaces_both_path_spellings() {
        let ctx = NormalizeCtx {
            root: PathBuf::from(r"D:\Temp\o824-fixture-abc"),
            data_root: Some(PathBuf::from(r"D:\Temp\data-root")),
            plan_id: "plan-20261101T121041Z-fce66dc1".to_owned(),
            install_id: "local-6f1c".to_owned(),
            created_at: "2026-11-01T12:10:41.5Z".to_owned(),
            expires_at: "2026-11-01T12:25:41.5Z".to_owned(),
        };
        let text = concat!(
            r#""path": "D:\\Temp\\o824-fixture-abc\\profiles\\desktop","#,
            "\n",
            r#""raw": "D:\Temp\o824-fixture-abc","#,
            "\n",
            r#""slash": "D:/Temp/o824-fixture-abc/profiles","#,
            "\n",
            r#""data": "D:\\Temp\\data-root\\journal","#,
            "\n",
            r#""id": "plan-20261101T121041Z-fce66dc1","#,
            "\n",
            r#""install": "local-6f1c","#,
            "\n",
            r#""createdAt": "2026-11-01T12:10:41.5Z","#,
            "\n",
            r#""expiresAt": "2026-11-01T12:25:41.5Z""#,
        );
        let normalized = normalize_text(text, &ctx);
        assert!(
            normalized.contains(r#"<ROOT>\\profiles\\desktop"#),
            "{normalized}"
        );
        assert!(normalized.contains(r#"<ROOT>","#), "{normalized}");
        assert!(normalized.contains(r#"<ROOT>/profiles"#), "{normalized}");
        assert!(normalized.contains(r#"<DATA>\\journal"#), "{normalized}");
        assert!(normalized.contains("<PLAN_ID>"));
        assert!(normalized.contains("<INSTALL_ID>"));
        assert!(!normalized.contains("2026-11-01T12:10:41.5Z"));
        assert_eq!(normalized.matches("<TIMESTAMP>").count(), 2);
    }

    #[test]
    fn compare_is_silent_when_equal_and_specific_when_not() {
        assert_eq!(compare("a\nb\n", "a\nb\n", "same"), Ok(()));

        let report = compare("a\nb\n", "a\nc\n", "different").expect_err("must differ");
        assert!(report.contains("golden mismatch: different"));
        assert!(report.contains("-b"), "{report}");
        assert!(report.contains("+c"), "{report}");
    }

    #[test]
    fn manifest_parser_reads_sections() {
        let text = r#"
[package]
name = "o824-demo"

[dependencies]
o824-errs.workspace = true
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
tempfile.workspace = true
"#;
        let manifest = parse_manifest(text);
        assert_eq!(manifest.name, "o824-demo");
        assert_eq!(manifest.dependencies, ["o824-errs", "serde"]);
        assert_eq!(manifest.dev_dependencies, ["tempfile"]);
    }

    #[test]
    fn forbidden_edges_are_detected() {
        let mut manifests = BTreeMap::new();
        manifests.insert(
            "o824-verify".to_owned(),
            Manifest {
                name: "o824-verify".to_owned(),
                dependencies: vec!["o824-plan".to_owned()],
                ..Manifest::default()
            },
        );
        manifests.insert(
            "o824-errs".to_owned(),
            Manifest {
                name: "o824-errs".to_owned(),
                dependencies: vec!["o824-model".to_owned()],
                ..Manifest::default()
            },
        );
        let violations = forbidden_edges(&manifests);
        assert!(violations
            .iter()
            .any(|edge| edge.crate_name == "o824-verify"));
        assert!(violations.iter().any(|edge| edge.crate_name == "o824-errs"));
    }
}
