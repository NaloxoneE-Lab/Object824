//! The crate graph is part of the architecture, not an accident of imports.
//!
//! Cargo catches cycles; it does not catch an edge that is legal but wrong —
//! `verify` reading the planner, or the leaf model crate reaching back into the
//! kernel. These tests read every `Cargo.toml` and fail on those edges.

use o824_conformance::{forbidden_edges, read_manifests};

#[test]
fn no_forbidden_dependency_edge_exists() {
    let manifests = read_manifests();
    assert!(
        !manifests.is_empty(),
        "no crate manifests found under crates/"
    );
    let violations = forbidden_edges(&manifests);
    assert!(
        violations.is_empty(),
        "forbidden dependency edges: {violations:#?}"
    );
}

#[test]
fn every_expected_crate_is_present() {
    let manifests = read_manifests();
    let expected = [
        "o824",
        "o824-apply",
        "o824-cli",
        "o824-conformance",
        "o824-diagnose",
        "o824-discovery",
        "o824-errs",
        "o824-execx",
        "o824-fixtures",
        "o824-lock",
        "o824-mock",
        "o824-model",
        "o824-plan",
        "o824-protocol",
        "o824-snapshot",
        "o824-state",
        "o824-verify",
        "o824-version",
    ];
    let mut missing = Vec::new();
    for name in expected {
        if !manifests.contains_key(name) {
            missing.push(name);
        }
    }
    assert!(missing.is_empty(), "missing crates: {missing:?}");
}

/// The two leaves sit below everything. `o824-errs` depends on nothing
/// internal; `o824-model` may only depend on it, because a finding carries an
/// error code (the reference implementation has the same edge).
#[test]
fn leaves_stay_at_the_bottom() {
    let manifests = read_manifests();

    let errs = manifests
        .get("o824-errs")
        .unwrap_or_else(|| panic!("o824-errs manifest missing"));
    let internal: Vec<&String> = errs
        .all_dependencies()
        .filter(|name| name.starts_with("o824-"))
        .collect();
    assert!(internal.is_empty(), "o824-errs depends on {internal:?}");

    let model = manifests
        .get("o824-model")
        .unwrap_or_else(|| panic!("o824-model manifest missing"));
    let internal: Vec<&String> = model
        .all_dependencies()
        .filter(|name| name.starts_with("o824-") && *name != "o824-errs")
        .collect();
    assert!(internal.is_empty(), "o824-model depends on {internal:?}");
}
