//! The manifest is only worth anything if it refuses the things it is supposed to refuse.
//! Every test here is a way a release can be wrong.

use std::fs;
use std::path::Path;

use o824_manifest::{build, new_key, public_hex, save, sign, verify_files, verify_signature};

/// A directory that removes itself. `tempfile` was the obvious choice and it is not used
/// here: it pulls a linking step this workspace's toolchain cannot run, and a test fixture is
/// not worth changing the toolchain for.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let unique = format!(
            "o824-manifest-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("scratch dir");
        Scratch(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

type TempDir = Scratch;

fn release(files: &[(&str, &str)]) -> TempDir {
    let directory = Scratch::new();
    for (path, contents) in files {
        let target = directory.path().join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(target, contents).expect("write");
    }
    directory
}

fn signed(files: &[(&str, &str)]) -> (TempDir, o824_manifest::Manifest, String) {
    let directory = release(files);
    let mut manifest = build(directory.path(), "0.1.0", "2026-11-04T10:00:00Z").expect("build");
    let key = new_key();
    sign(&mut manifest, &key).expect("sign");
    let public = public_hex(&key);
    (directory, manifest, public)
}

#[test]
fn a_signed_manifest_verifies_and_matches_its_files() {
    let (directory, manifest, public) =
        signed(&[("o824.exe", "the launcher"), ("docs/readme.txt", "hello")]);

    verify_signature(&manifest, Some(&public)).expect("signature");
    let report = verify_files(&manifest, directory.path()).expect("files");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.checked, 2);
    assert!(manifest
        .files
        .iter()
        .any(|entry| entry.path == "docs/readme.txt"));
}

#[test]
fn a_changed_byte_is_reported_by_name() {
    let (directory, manifest, _) = signed(&[("o824.exe", "the launcher")]);
    fs::write(
        directory.path().join("o824.exe"),
        "the launcher, but different",
    )
    .expect("write");

    let report = verify_files(&manifest, directory.path()).expect("files");
    assert_eq!(report.changed, vec!["o824.exe".to_owned()]);
    assert!(!report.is_clean());
}

#[test]
fn editing_the_manifest_breaks_the_signature() {
    let (directory, mut manifest, public) = signed(&[("o824.exe", "the launcher")]);
    // Point the manifest at bytes that are not on disk — the exact move a hostile mirror
    // would make: swap the binary, then rewrite the hash.
    manifest.files[0].sha256 = "00".repeat(32);

    let error =
        verify_signature(&manifest, Some(&public)).expect_err("a rewritten hash must not verify");
    assert_eq!(error.code, o824_errs::Code::StateDrift);
    // And the file check now disagrees with the manifest as well.
    let report = verify_files(&manifest, directory.path()).expect("files");
    assert_eq!(report.changed, vec!["o824.exe".to_owned()]);
}

#[test]
fn a_manifest_signed_by_another_key_is_refused() {
    let (_, manifest, _) = signed(&[("o824.exe", "the launcher")]);
    let other = public_hex(&new_key());

    let error = verify_signature(&manifest, Some(&other)).expect_err("wrong key");
    assert_eq!(error.code, o824_errs::Code::StateDrift);
    assert!(
        error.fallback.contains("not by the expected key"),
        "{}",
        error.fallback
    );
}

#[test]
fn a_missing_file_is_named() {
    let (directory, manifest, _) = signed(&[("o824.exe", "a"), ("o824-kernel.exe", "b")]);
    fs::remove_file(directory.path().join("o824-kernel.exe")).expect("remove");

    let report = verify_files(&manifest, directory.path()).expect("files");
    assert_eq!(report.missing, vec!["o824-kernel.exe".to_owned()]);
}

#[test]
fn an_extra_file_is_named() {
    let (directory, manifest, _) = signed(&[("o824.exe", "a")]);
    fs::write(directory.path().join("planted.dll"), "not in the manifest").expect("write");

    let report = verify_files(&manifest, directory.path()).expect("files");
    assert_eq!(report.unexpected, vec!["planted.dll".to_owned()]);
}

#[test]
fn an_unsigned_manifest_says_so() {
    let directory = release(&[("o824.exe", "a")]);
    let manifest = build(directory.path(), "0.1.0", "2026-11-04T10:00:00Z").expect("build");

    let error = verify_signature(&manifest, None).expect_err("unsigned");
    assert_eq!(error.code, o824_errs::Code::StateDrift);
    assert!(
        error.fallback.contains("no signature"),
        "{}",
        error.fallback
    );
}

#[test]
fn the_manifest_skips_itself_and_is_stable_across_builds() {
    let directory = release(&[
        ("b.txt", "second"),
        ("a.txt", "first"),
        ("nested/c.txt", "third"),
    ]);
    let first = build(directory.path(), "0.1.0", "2026-11-04T10:00:00Z").expect("build");
    save(&first, &directory.path().join("manifest.json")).expect("save");

    // Writing the manifest into the tree must not change what it describes.
    let second = build(directory.path(), "0.1.0", "2026-11-04T10:00:00Z").expect("build again");
    assert_eq!(first.files, second.files, "the manifest described itself");

    let paths: Vec<&str> = second
        .files
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["a.txt", "b.txt", "nested/c.txt"],
        "sorted, forward slashes"
    );
}

#[test]
fn the_signed_bytes_do_not_include_the_signature() {
    let (_, mut manifest, _) = signed(&[("o824.exe", "a")]);
    let before = o824_manifest::signed_bytes(&manifest).expect("bytes");
    let signature = manifest.signature.clone().expect("signed");
    manifest.signature = None;
    let after = o824_manifest::signed_bytes(&manifest).expect("bytes");
    assert_eq!(before, after, "the signature is not part of what is signed");
    assert!(signature.value.len() == 128, "ed25519 is 64 bytes of hex");
}

#[test]
fn a_manifest_on_disk_round_trips() {
    let (directory, manifest, public) = signed(&[("o824.exe", "a")]);
    let path = directory.path().join("manifest.json");
    save(&manifest, &path).expect("save");
    let loaded = o824_manifest::load(&path).expect("load");
    assert_eq!(loaded, manifest);
    verify_signature(&loaded, Some(&public)).expect("signature survives the round trip");
    assert!(path.exists());
}

#[test]
fn a_file_that_is_not_a_manifest_is_refused() {
    let directory = release(&[("manifest.json", "{\"hello\":1}")]);
    let error =
        o824_manifest::load(&directory.path().join("manifest.json")).expect_err("not a manifest");
    assert_eq!(error.code, o824_errs::Code::Usage);
}

#[test]
fn a_symbolic_link_is_not_part_of_the_release() {
    let directory = release(&[("o824.exe", "a")]);
    let link = directory.path().join("link.exe");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(directory.path().join("o824.exe"), &link).is_ok();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(directory.path().join("o824.exe"), &link).is_ok();

    let manifest = build(directory.path(), "0.1.0", "2026-11-04T10:00:00Z").expect("build");
    if made {
        assert!(
            !manifest.files.iter().any(|entry| entry.path == "link.exe"),
            "a link describes another machine's disk"
        );
    }
    assert!(Path::new(&directory.path().join("o824.exe")).exists());
}
