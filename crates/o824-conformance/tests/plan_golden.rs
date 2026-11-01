//! The eight plan goldens, produced by the real implementation.
//!
//! This is gate G2: on every fixture tree, `repair --dry-run --json` must
//! reproduce the contract file the reference implementation recorded — byte for
//! byte, after normalization. It also proves determinism, which is the extra
//! claim a plan makes over a diagnosis: the same tree at the same instant must
//! produce the same steps.

use std::path::{Path, PathBuf};

use o824_conformance::{assert_golden, golden_file, load, normalize_text, NormalizeCtx, KINDS};
use serde_json::Value;

/// What one dry run produced.
struct DryRun {
    root: PathBuf,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// Runs `repair --root <root> --home <root> --dry-run --json` in a data root the
/// tree needs.
///
/// The journal lives under the data root, not under the installation, so a tree
/// with an unfinished install has its journal copied into a data root of its own
/// — exactly what the reference implementation's golden test does.
fn dry_run(kind: &str) -> (DryRun, o824_fixtures::TempDir) {
    let fixture = o824_fixtures::build(kind).unwrap_or_else(|error| panic!("{kind}: {error}"));
    let root = fixture.root().to_path_buf();

    let data_root = o824_fixtures::TempDir::new("o824-plan-data-").expect("data root");
    let journal = root.join("journal");
    if journal.is_dir() {
        let target = data_root.path().join("journal");
        std::fs::create_dir_all(&target).expect("create journal dir");
        for entry in std::fs::read_dir(&journal).expect("read journal").flatten() {
            std::fs::copy(entry.path(), target.join(entry.file_name())).expect("copy journal");
        }
    }

    let args: Vec<String> = [
        "--root",
        &root.display().to_string(),
        "--home",
        &root.display().to_string(),
        "--dry-run",
        "--json",
    ]
    .iter()
    .map(|argument| (*argument).to_owned())
    .collect();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code =
        o824_cli::repair::run_with(&args, &mut stdout, &mut stderr, Some(data_root.path()));

    let run = DryRun {
        root,
        exit_code,
        stdout: String::from_utf8(stdout).expect("utf8 stdout"),
        stderr: String::from_utf8(stderr).expect("utf8 stderr"),
    };
    (run, data_root)
}

/// Pretty-prints the CLI's compact JSON with the shape the golden files use.
fn pretty(compact: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(compact)
        .unwrap_or_else(|error| panic!("repair did not print JSON: {error}\n{compact}"));
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("serialize")
    )
}

/// Builds the normalizer for one plan document from the values the plan itself
/// carries: the ids and instants are exactly what a golden cannot contain.
fn context_for(document: &Value, root: &Path) -> NormalizeCtx {
    let text = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    NormalizeCtx {
        root: root.to_path_buf(),
        data_root: None,
        plan_id: text("id"),
        install_id: text("installId"),
        created_at: text("createdAt"),
        expires_at: text("expiresAt"),
    }
}

#[test]
fn every_plan_golden_is_reproduced() {
    for kind in KINDS {
        let (run, _data_root) = dry_run(kind);
        assert_eq!(
            run.exit_code, 0,
            "{kind}: a dry run must exit 0; stderr: {}",
            run.stderr
        );
        assert_eq!(
            run.stderr, "",
            "{kind}: repair wrote to stderr: {}",
            run.stderr
        );

        let actual = pretty(&run.stdout);
        let document: Value = serde_json::from_str(&actual).expect("plan json");
        let golden = load(&golden_file(kind, "plan"));

        let normalized = normalize_text(&actual, &context_for(&document, &run.root));
        assert_golden(&golden, &normalized, &format!("{kind}.plan"));
    }
}

/// A plan promises determinism: the same tree at the same instant is the same
/// plan. Two fresh trees must therefore normalize to identical documents.
#[test]
fn two_fresh_trees_produce_the_same_plan() {
    for kind in KINDS {
        let (first, _first_root) = dry_run(kind);
        let (second, _second_root) = dry_run(kind);

        let first_document: Value = serde_json::from_str(&pretty(&first.stdout)).expect("first");
        let second_document: Value = serde_json::from_str(&pretty(&second.stdout)).expect("second");
        let first_text = normalize_text(
            &pretty(&first.stdout),
            &context_for(&first_document, &first.root),
        );
        let second_text = normalize_text(
            &pretty(&second.stdout),
            &context_for(&second_document, &second.root),
        );
        assert_eq!(
            first_text, second_text,
            "{kind}: the plan is not deterministic"
        );
    }
}

/// The plan a golden pins must never carry an absolute path that normalization
/// would hide: every path in a step is the fixture root plus a relative suffix,
/// and every fingerprint key is root-relative.
#[test]
fn plan_paths_are_root_anchored() {
    for kind in KINDS {
        let (run, _data_root) = dry_run(kind);
        let document: Value = serde_json::from_str(&pretty(&run.stdout)).expect("plan json");
        let root = run.root.display().to_string();

        for step in document["steps"].as_array().expect("steps array") {
            for field in ["path", "from"] {
                let Some(value) = step.get(field).and_then(Value::as_str) else {
                    continue;
                };
                if value.is_empty() {
                    continue;
                }
                assert!(
                    value.to_lowercase().starts_with(&root.to_lowercase()),
                    "{kind}: {field} {value:?} is not under the installation root"
                );
            }
        }
        for key in document["fingerprint"]
            .as_object()
            .expect("fingerprint object")
            .keys()
        {
            assert!(
                !key.contains(':'),
                "{kind}: fingerprint key {key:?} is absolute"
            );
            assert!(
                !key.contains('\\'),
                "{kind}: fingerprint key {key:?} is not slash-separated"
            );
        }
    }
}

#[test]
fn the_steps_are_the_ones_the_findings_rule() {
    // A compact statement of the mapping gate G2 pins: which fixtures repair, and
    // with which kind of step.
    let expected: [(&str, &[&str]); 8] = [
        ("ok", &[]),
        ("broken-link", &[]),
        ("missing-bundle", &[]),
        ("dup-loader", &[]),
        ("parse-error", &[]),
        ("half-install", &[]),
        ("dangling-bridge", &["rebuild-bridge"]),
        ("git-dep", &["allow-builds"]),
    ];
    for (kind, kinds) in expected {
        let (run, _data_root) = dry_run(kind);
        let document: Value = serde_json::from_str(&pretty(&run.stdout)).expect("plan json");
        let actual: Vec<&str> = document["steps"]
            .as_array()
            .expect("steps array")
            .iter()
            .map(|step| step["kind"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(actual, kinds, "{kind}");
    }
}
