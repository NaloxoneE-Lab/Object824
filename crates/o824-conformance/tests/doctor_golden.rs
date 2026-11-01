//! The eight doctor goldens, produced by the real implementation.
//!
//! This is gate G1: on every fixture tree, `doctor --json` must reproduce the
//! contract file the reference implementation recorded — byte for byte, after
//! normalization. Until this test exists there is no proof that the seven checks
//! mean what the Go version meant.

use std::path::{Path, PathBuf};

use o824_conformance::{assert_golden, golden_file, load, normalize_text, NormalizeCtx, KINDS};

/// What one doctor run produced.
struct Diagnosis {
    root: PathBuf,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// Runs `doctor --root <root> --home <root> --json` in the data root the tree
/// needs.
///
/// The journal lives under the data root, not under the installation, so a tree
/// with an unfinished install has its journal copied into a data root of its own
/// — exactly what the reference implementation's golden test does.
fn diagnose(kind: &str) -> (Diagnosis, o824_fixtures::TempDir) {
    let fixture = o824_fixtures::build(kind).unwrap_or_else(|error| panic!("{kind}: {error}"));
    let root = fixture.root().to_path_buf();

    let data_root = o824_fixtures::TempDir::new("o824-data-").expect("data root");
    let journal = root.join("journal");
    if journal.is_dir() {
        let target = data_root.path().join("journal");
        std::fs::create_dir_all(&target).expect("create journal dir");
        for entry in std::fs::read_dir(&journal).expect("read journal").flatten() {
            std::fs::copy(entry.path(), target.join(entry.file_name())).expect("copy journal");
        }
    }

    // The subcommand entry point takes the arguments *after* the command name.
    let args: Vec<String> = [
        "--root",
        &root.display().to_string(),
        "--home",
        &root.display().to_string(),
        "--json",
    ]
    .iter()
    .map(|argument| (*argument).to_owned())
    .collect();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let exit_code =
        o824_cli::doctor::run_with(&args, &mut stdout, &mut stderr, Some(data_root.path()));

    let diagnosis = Diagnosis {
        root,
        exit_code,
        stdout: String::from_utf8(stdout).expect("utf8 stdout"),
        stderr: String::from_utf8(stderr).expect("utf8 stderr"),
    };
    // The temp dir must outlive the diagnosis, because the diagnosis borrows the
    // root path for its comparisons; the caller keeps it alive.
    (diagnosis, data_root)
}

/// Pretty-prints the CLI's compact JSON with the same shape the golden files
/// use: two-space indent, LF, one trailing newline.
fn pretty(compact: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(compact)
        .unwrap_or_else(|error| panic!("doctor did not print JSON: {error}\n{compact}"));
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("serialize")
    )
}

#[test]
fn every_doctor_golden_is_reproduced() {
    for kind in KINDS {
        let (diagnosis, _data_root) = diagnose(kind);
        assert_eq!(
            diagnosis.stderr, "",
            "{kind}: doctor wrote to stderr: {}",
            diagnosis.stderr
        );

        let actual = pretty(&diagnosis.stdout);
        let path = golden_file(kind, "doctor");
        let golden = load(&path);

        // Doctor output must be machine-independent by construction: no
        // absolute path may leak into a finding parameter.
        let ctx = NormalizeCtx::for_root(&diagnosis.root);
        let normalized = normalize_text(&actual, &ctx);
        assert_eq!(
            normalized, actual,
            "{kind}: doctor output carries a machine-specific value; every finding \
             parameter has to be root-relative"
        );

        assert_golden(&golden, &actual, &format!("{kind}.doctor"));
    }
}

#[test]
fn the_exit_code_reports_whether_there_were_findings() {
    for kind in KINDS {
        let (diagnosis, _data_root) = diagnose(kind);
        let expected = if kind == "ok" { 0 } else { 1 };
        assert_eq!(
            diagnosis.exit_code, expected,
            "{kind}: doctor exit code (findings are a diagnosis, not an error)"
        );
    }
}

#[test]
fn the_ok_tree_reports_no_findings_at_all() {
    let (diagnosis, _data_root) = diagnose("ok");
    assert_eq!(
        diagnosis.stdout.trim(),
        r#"{"envelope":"dsh-doctor/v1","findings":[]}"#
    );
}

/// Two runs over two freshly built trees must agree: this is what turns an
/// accidental machine-specific value into a hard failure instead of a flake.
#[test]
fn two_fresh_trees_produce_the_same_document() {
    for kind in KINDS {
        let (first, _first_root) = diagnose(kind);
        let (second, _second_root) = diagnose(kind);
        assert_eq!(
            pretty(&first.stdout),
            pretty(&second.stdout),
            "{kind}: two trees produced different documents"
        );
    }
}

/// The normalizer must handle the fixture root in all three of its Windows
/// spellings, so a future parameter that does carry a path is still comparable.
#[test]
fn the_normalizer_handles_the_root_the_harness_uses() {
    let root = Path::new(r"D:\Temp\o824-fixture-abc");
    let ctx = NormalizeCtx::for_root(root);
    let text = r#"{"a":"D:\\Temp\\o824-fixture-abc\\x","b":"D:/Temp/o824-fixture-abc/y"}"#;
    let normalized = normalize_text(text, &ctx);
    assert_eq!(normalized, r#"{"a":"<ROOT>\\x","b":"<ROOT>/y"}"#);
}
