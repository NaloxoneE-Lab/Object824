//! The first fixture tree, wired end to end: `fixtures::build("ok")` produces a
//! tree that `discovery::validate` calls healthy with the DSH version — not the
//! Electron version that sits in the same directory.

use o824_conformance::KINDS;

#[test]
fn the_ok_tree_validates_as_healthy() {
    let fixture = o824_fixtures::build("ok").expect("build the ok fixture");
    // The fixture root doubles as its DSH home: resolving the ambient ~/.dsh would
    // make this assertion describe the machine, not the tree.
    let install = o824_discovery::validate_with(fixture.root(), fixture.root(), None)
        .expect("the ok tree must validate");

    assert_eq!(install.health, "ok");
    assert_eq!(install.version, "0.2.1-rc.3");
    assert_eq!(install.kind, "desktop");
    assert_eq!(install.profiles, vec!["desktop".to_owned()]);
}

#[test]
fn the_electron_version_in_the_version_file_is_ignored() {
    let fixture = o824_fixtures::build("ok").expect("build the ok fixture");
    let marker = std::fs::read_to_string(fixture.root().join("version")).expect("read version");
    assert_eq!(marker.trim(), "44.7.0");

    let install = o824_discovery::validate(fixture.root()).expect("validate");
    assert_ne!(install.version, marker.trim());
}

#[test]
fn all_eight_kinds_build_and_the_half_install_has_no_asar() {
    for kind in KINDS {
        let fixture = o824_fixtures::build(kind).unwrap_or_else(|error| panic!("{kind}: {error}"));
        let asar = fixture.root().join("resources").join("app.asar");
        assert_eq!(
            asar.exists(),
            kind != "half-install",
            "{kind} asar presence is wrong"
        );
    }
}

#[test]
fn a_tree_that_is_not_an_install_is_reported_as_e_install_dir() {
    let dir = o824_fixtures::TempDir::new("o824-test-").expect("temp dir");
    let error = o824_discovery::validate(dir.path()).expect_err("an empty directory");
    assert_eq!(error.code.as_str(), "E_INSTALL_DIR");
}
