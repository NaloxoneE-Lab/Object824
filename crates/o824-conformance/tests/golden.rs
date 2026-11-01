//! The golden harness itself: the files parse, the pipeline can pass, and a
//! deliberately broken document is caught with a usable diff.
//!
//! At M0 there is no `verify` yet, so the "implementation" is a stub that
//! replays the contract. That is deliberate: what has to be trustworthy before
//! any business logic exists is the comparison, not the producer.

use std::path::PathBuf;

use o824_conformance::{
    assert_golden, compare, golden_dir, golden_file, load, normalize_text, NormalizeCtx, KINDS,
};
use serde_json::Value;

/// Every contract document: eight kinds times two documents.
fn all_documents() -> Vec<(String, String, PathBuf)> {
    let mut documents = Vec::new();
    for kind in KINDS {
        for document in ["doctor", "plan"] {
            documents.push((
                kind.to_owned(),
                document.to_owned(),
                golden_file(kind, document),
            ));
        }
    }
    documents
}

/// Fills a golden document's placeholders with the values a real run would
/// print, so normalization has something to put back.
fn concretize(golden: &str, ctx: &NormalizeCtx) -> String {
    let mut concrete = golden
        .replace("<ROOT>", &ctx.root.display().to_string())
        .replace("<PLAN_ID>", &ctx.plan_id)
        .replace("<INSTALL_ID>", &ctx.install_id);
    if let Some(data_root) = &ctx.data_root {
        concrete = concrete.replace("<DATA>", &data_root.display().to_string());
    }
    // The plan carries two timestamps under one placeholder; they are distinct
    // in a real document, and normalization must restore both.
    if let Some(position) = concrete.find("<TIMESTAMP>") {
        concrete.replace_range(position..position + "<TIMESTAMP>".len(), &ctx.created_at);
        concrete = concrete.replacen("<TIMESTAMP>", &ctx.expires_at, 1);
    }
    concrete
}

#[test]
fn all_sixteen_golden_files_exist_and_parse() {
    let documents = all_documents();
    assert_eq!(documents.len(), 16);

    for (kind, document, path) in documents {
        assert!(path.is_file(), "missing golden file {}", path.display());
        let text = load(&path);
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
        assert!(value.is_object(), "{} is not an object", path.display());

        let object = value.as_object().expect("object");
        let keys: Vec<&str> = object.keys().map(String::as_str).collect();
        if document == "doctor" {
            assert_eq!(
                keys,
                ["envelope", "findings"],
                "{kind}.doctor.json key order"
            );
        } else {
            assert_eq!(
                keys,
                [
                    "id",
                    "installId",
                    "root",
                    "createdAt",
                    "expiresAt",
                    "steps",
                    "fingerprint"
                ],
                "{kind}.plan.json key order"
            );
        }
    }
}

#[test]
fn the_golden_directory_holds_exactly_sixteen_contract_files() {
    let entries = std::fs::read_dir(golden_dir()).expect("read spec/golden");
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 16, "{names:?}");
}

/// The stub replays the contract, so the comparison must pass untouched.
#[test]
fn stub_replay_matches_every_golden() {
    for (kind, document, path) in all_documents() {
        let expected = load(&path);
        assert_golden(&expected, &expected, &format!("{kind}.{document}"));
    }
}

/// The same replay, but with every machine-dependent value substituted for a
/// concrete one first: normalization has to put the placeholders back.
#[test]
fn normalization_restores_concrete_values() {
    let ctx = NormalizeCtx {
        root: PathBuf::from(r"D:\Temp\o824-fixture-3f2a"),
        data_root: Some(PathBuf::from(r"D:\Temp\o824-data-root")),
        plan_id: "plan-20261101T121041Z-fce66dc1".to_owned(),
        install_id: "local-6f1c1a0d5e6b7c8d".to_owned(),
        created_at: "2026-11-01T12:10:41.5123456Z".to_owned(),
        expires_at: "2026-11-01T12:25:41.5123456Z".to_owned(),
    };

    let mut with_placeholders = 0;
    for (kind, document, path) in all_documents() {
        let golden = load(&path);
        let concrete = concretize(&golden, &ctx);
        if concrete == golden {
            // A document can be entirely machine-independent: a clean tree's
            // doctor output names no path at all.
            continue;
        }
        with_placeholders += 1;

        let normalized = normalize_text(&concrete, &ctx);
        assert_golden(&golden, &normalized, &format!("{kind}.{document}"));
    }
    assert_eq!(
        with_placeholders, 8,
        "only the eight plan documents carry machine-dependent values: doctor findings \
         report profile-relative paths, so they normalise to themselves"
    );
}

/// The harness must fail — and say which line — when one field changes.
#[test]
fn a_mutated_field_fails_with_a_diff() {
    let path = golden_file("git-dep", "doctor");
    let golden = load(&path);
    let mut value: Value = serde_json::from_str(&golden).expect("parse golden");
    value["findings"][0]["severity"] = Value::from("info");
    let mutated = format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("serialize")
    );

    let report = compare(&golden, &mutated, "git-dep.doctor (mutated)")
        .expect_err("a changed field must not pass");
    assert!(report.contains("golden mismatch"), "{report}");
    // The diff must show the changed line on both sides, sign and all.
    assert!(
        report.contains("-      \"severity\": \"error\","),
        "{report}"
    );
    assert!(
        report.contains("+      \"severity\": \"info\","),
        "{report}"
    );
}

/// `assert_golden` is the panicking wrapper; it must panic with the diff.
#[test]
fn assert_golden_panics_with_the_diff() {
    let result = std::panic::catch_unwind(|| {
        assert_golden("{\n  \"a\": 1\n}\n", "{\n  \"a\": 2\n}\n", "unit");
    });
    let payload = result.expect_err("must panic");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "non-string panic payload".to_owned());
    assert!(message.contains("\"a\": 1"), "{message}");
    assert!(message.contains("\"a\": 2"), "{message}");
}
