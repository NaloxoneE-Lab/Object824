//! `o824 update --check` as a caller sees it.
//!
//! The command exists to answer three questions before anything is ever replaced: is this
//! manifest signed by the key I trust, is it newer, and which local files would change. Each
//! of those has a way to be wrong, and each wrong way has a test.

use std::fs;

use o824_manifest::{build, new_key, public_hex, save, sign};

use super::update_cmd;
use crate::{
    EXIT_CONFIRMATION_REQUIRED, EXIT_FINDINGS, EXIT_OK, EXIT_PERMISSION, EXIT_STATE_DRIFT,
    EXIT_USAGE,
};

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

/// The trust rule, tested directly because the built-in key is a compile-time constant: a
/// test cannot rebuild the binary with a key, but it can hand the resolver one.
#[test]
fn an_explicit_key_wins_over_the_built_in_one() {
    let trust = super::update_cmd::resolve_trust(Some("aa".repeat(32)), Some("bb"));
    assert_eq!(trust, super::update_cmd::Trust::Named("aa".repeat(32)));
}

#[test]
fn without_an_argument_the_built_in_key_is_used() {
    let trust = super::update_cmd::resolve_trust(None, Some("bb".repeat(32).leak()));
    assert_eq!(trust, super::update_cmd::Trust::BuiltIn("bb".repeat(32)));
}

#[test]
fn a_build_without_a_key_trusts_nobody_but_itself() {
    let trust = super::update_cmd::resolve_trust(None, None);
    assert_eq!(trust, super::update_cmd::Trust::Nobody);
}

#[test]
fn an_empty_argument_falls_through_to_the_built_in_key() {
    let trust = super::update_cmd::resolve_trust(Some(String::new()), Some("cc"));
    assert_eq!(trust, super::update_cmd::Trust::BuiltIn("cc".to_owned()));
}

/// This development build has no release key compiled in, so the honest wording must appear.
/// A release build flips this expectation by setting `O824_RELEASE_KEY`.
#[test]
fn a_development_build_says_it_vouches_for_nothing() {
    let scratch = Scratch::new("devbuild");
    let (manifest, _, _) = published(&scratch, "9.9.9");

    let (code, out, _) = run(&["--check", "--manifest", &manifest, "--version", "0.1.0"]);
    assert_eq!(code, EXIT_FINDINGS, "{out}");
    if o824_manifest::built_in_key().is_none() {
        assert!(out.contains("self-signed only"), "{out}");
    } else {
        assert!(out.contains("built into this binary"), "{out}");
    }
}

/// The manifest arrives over the network. A path that walks out of the target directory is the
/// difference between a downloader and a way to write files wherever the user can write.
#[test]
fn a_manifest_path_cannot_escape_the_target() {
    let allowed = [
        "o824.exe",
        "o824-gui.exe",
        "docs/readme.txt",
        "./o824.exe",
        "a/b/c.dll",
    ];
    for path in allowed {
        assert!(
            super::update_cmd::safe_relative(path).is_some(),
            "{path:?} should be accepted"
        );
    }

    let refused = [
        "../o824.exe",
        "docs/../../escape.exe",
        "../../../../Windows/System32/evil.dll",
        "/absolute/path.dll",
        "C:/Windows/System32/evil.dll",
        "c:relative.dll",
        "..",
        ".",
        "",
        "docs/..",
    ];
    for path in refused {
        assert!(
            super::update_cmd::safe_relative(path).is_none(),
            "{path:?} must be refused"
        );
    }
}

/// Windows separators are normalised before the check, so a manifest written for this platform
/// cannot sneak past it.
#[test]
fn backslashes_are_normalised_before_the_check() {
    assert!(super::update_cmd::safe_relative(r"docs\readme.txt").is_some());
    assert!(super::update_cmd::safe_relative(r"docs\..\..\evil.exe").is_none());
}

#[test]
fn fetching_without_the_urls_is_a_usage_error() {
    let scratch = Scratch::new("fetch-usage");
    let (code, _, err) = run(&["--fetch", "--into", scratch.path().to_str().expect("path")]);
    assert_eq!(code, EXIT_USAGE, "{err}");
    assert!(err.contains("--manifest-url"), "{err}");
}

/// The swap is the moment a manifest stops being advice. Its plan must never write outside the
/// installation, and must keep a backup name for every file it is about to replace.
#[test]
fn a_swap_plan_keeps_every_file_inside_the_installation() {
    let scratch = Scratch::new("plan");
    let staged = scratch.path().join("staged");
    let target = scratch.path().join("installed");
    fs::create_dir_all(&staged).expect("staged");

    let mut manifest =
        o824_manifest::build(&staged, "0.2.0", "2026-11-05T00:00:00Z").expect("build");
    // A hostile entry sits next to two honest ones.
    manifest.files.push(o824_manifest::Entry {
        path: "../evil.dll".to_owned(),
        sha256: "00".repeat(32),
        size: 1,
    });
    manifest.files.push(o824_manifest::Entry {
        path: "o824.exe".to_owned(),
        sha256: "00".repeat(32),
        size: 1,
    });

    let plan = super::update_cmd::swap_plan(&manifest, &staged, &target);
    assert_eq!(plan.len(), 1, "only the honest entry survives: {plan:?}");
    let step = &plan[0];
    assert!(
        step.to.starts_with(&target),
        "{:?} escapes the installation",
        step.to
    );
    assert_eq!(
        step.backup.file_name().and_then(|name| name.to_str()),
        Some("o824.exe.bak")
    );
    assert_eq!(step.from, staged.join("o824.exe"));
}

#[test]
fn applying_without_yes_is_refused_before_anything_is_checked() {
    let scratch = Scratch::new("apply-yes");
    let (code, _, err) = run(&[
        "--apply",
        "--staged",
        scratch.path().to_str().expect("path"),
        "--target",
        scratch.path().join("installed").to_str().expect("path"),
    ]);
    assert_eq!(code, EXIT_CONFIRMATION_REQUIRED, "{err}");
    assert!(err.contains("--yes"), "{err}");
}

#[test]
fn applying_a_folder_without_a_manifest_fails() {
    let scratch = Scratch::new("apply-nomanifest");
    let (code, _, err) = run(&[
        "--apply",
        "--yes",
        "--staged",
        scratch.path().to_str().expect("path"),
        "--target",
        scratch.path().join("installed").to_str().expect("path"),
    ]);
    assert_eq!(code, EXIT_PERMISSION, "{err}");
}
