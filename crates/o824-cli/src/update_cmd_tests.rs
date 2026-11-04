//! `o824 update --check` as a caller sees it.
//!
//! The command exists to answer three questions before anything is ever replaced: is this
//! manifest signed by the key I trust, is it newer, and which local files would change. Each
//! of those has a way to be wrong, and each wrong way has a test.

use std::fs;

use o824_manifest::{build, new_key, public_hex, save, sign};

use super::update_cmd;
use crate::{EXIT_FINDINGS, EXIT_OK, EXIT_PERMISSION, EXIT_STATE_DRIFT, EXIT_USAGE};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!(
            "o824-update-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("scratch");
        Scratch(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> (i32, String, String) {
    let owned: Vec<String> = args.iter().map(|argument| (*argument).to_owned()).collect();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let code = update_cmd::run(&owned, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

/// A published release and a signed manifest for it; returns the paths and the public key.
fn published(scratch: &Scratch, version: &str) -> (String, String, String) {
    let release = scratch.path().join("release");
    fs::create_dir_all(&release).expect("release dir");
    fs::write(release.join("o824.exe"), format!("build {version}")).expect("write");

    let mut manifest = build(&release, version, "2026-11-04T12:00:00Z").expect("build");
    let key = new_key();
    sign(&mut manifest, &key).expect("sign");
    let path = scratch.path().join("manifest.json");
    save(&manifest, &path).expect("save");

    (
        path.to_str().expect("path").to_owned(),
        release.to_str().expect("path").to_owned(),
        public_hex(&key),
    )
}

#[test]
fn a_newer_signed_release_is_reported() {
    let scratch = Scratch::new("newer");
    let (manifest, _, public) = published(&scratch, "9.9.9");

    let (code, out, err) = run(&[
        "--check",
        "--manifest",
        &manifest,
        "--public-key",
        &public,
        "--version",
        "0.1.0",
    ]);
    assert_eq!(code, EXIT_FINDINGS, "{out}{err}");
    assert!(out.contains("signature: ok"), "{out}");
    assert!(out.contains("a newer release is published"), "{out}");
    assert!(out.contains("nothing downloaded"), "{out}");
}

#[test]
fn the_same_version_is_not_newer() {
    let scratch = Scratch::new("same");
    let (manifest, _, public) = published(&scratch, "0.1.0");

    let (code, out, _) = run(&[
        "--check",
        "--manifest",
        &manifest,
        "--public-key",
        &public,
        "--version",
        "0.1.0",
    ]);
    assert_eq!(code, EXIT_OK, "{out}");
    assert!(out.contains("nothing newer"), "{out}");
}

#[test]
fn an_older_release_is_not_an_update() {
    let scratch = Scratch::new("older");
    let (manifest, _, public) = published(&scratch, "0.1.0");

    let (code, out, _) = run(&[
        "--check",
        "--manifest",
        &manifest,
        "--public-key",
        &public,
        "--version",
        "0.2.0",
    ]);
    assert_eq!(code, EXIT_OK, "{out}");
}

#[test]
fn a_manifest_signed_by_another_key_is_refused_before_versions_are_compared() {
    let scratch = Scratch::new("wrongkey");
    let (manifest, _, _) = published(&scratch, "9.9.9");
    let stranger = public_hex(&new_key());

    let (code, out, err) = run(&[
        "--check",
        "--manifest",
        &manifest,
        "--public-key",
        &stranger,
        "--version",
        "0.1.0",
    ]);
    assert_eq!(code, EXIT_STATE_DRIFT, "{out}{err}");
    assert!(err.contains("not by the expected key"), "{err}");
    assert!(
        !out.contains("newer release"),
        "it must not get that far: {out}"
    );
}

#[test]
fn without_a_trusted_key_the_check_says_what_it_proves() {
    let scratch = Scratch::new("selfsigned");
    let (manifest, _, _) = published(&scratch, "9.9.9");

    let (code, out, _) = run(&["--check", "--manifest", &manifest, "--version", "0.1.0"]);
    assert_eq!(code, EXIT_FINDINGS, "{out}");
    assert!(
        out.contains("self-signed only"),
        "the difference is stated, not hidden: {out}"
    );
}

#[test]
fn the_installed_directory_shows_which_files_differ() {
    let scratch = Scratch::new("differs");
    let (manifest, release, public) = published(&scratch, "9.9.9");
    fs::write(
        std::path::Path::new(&release).join("o824.exe"),
        "an older build",
    )
    .expect("write");

    let (code, out, _) = run(&[
        "--check",
        "--manifest",
        &manifest,
        "--public-key",
        &public,
        "--installed",
        &release,
        "--version",
        "0.1.0",
    ]);
    assert_eq!(code, EXIT_FINDINGS, "{out}");
    assert!(out.contains("differs:    o824.exe"), "{out}");
    assert!(out.contains("changes nothing"), "{out}");
}

#[test]
fn replacing_is_not_a_flag_that_exists() {
    let scratch = Scratch::new("apply");
    let (manifest, _, public) = published(&scratch, "9.9.9");

    let (code, _, err) = run(&["--manifest", &manifest, "--public-key", &public]);
    assert_eq!(code, EXIT_USAGE, "{err}");
    assert!(err.contains("only checks"), "{err}");
    assert!(err.contains("not implemented"), "{err}");
}

#[test]
fn a_missing_manifest_cannot_be_read() {
    let scratch = Scratch::new("missing");
    let path = scratch.path().join("nope.json");
    let (code, _, err) = run(&["--check", "--manifest", path.to_str().expect("path")]);
    assert_eq!(code, EXIT_PERMISSION, "{err}");
}

#[test]
fn the_json_form_carries_the_same_answers() {
    let scratch = Scratch::new("json");
    let (manifest, _, public) = published(&scratch, "9.9.9");

    let (code, out, _) = run(&[
        "--check",
        "--manifest",
        &manifest,
        "--public-key",
        &public,
        "--version",
        "0.1.0",
        "--json",
    ]);
    assert_eq!(code, EXIT_FINDINGS, "{out}");
    let value: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
    assert_eq!(value["published"], "9.9.9");
    assert_eq!(value["current"], "0.1.0");
    assert_eq!(value["newer"], true);
    assert_eq!(value["trusted"], true);
}
