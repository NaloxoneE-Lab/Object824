//! The three manifest commands, as a caller sees them: exit codes and the words on screen.
//!
//! These exist because the exit codes are the contract a script switches on, and because a
//! verification tool that says "ok" when it should say "changed" is worse than no tool.

use std::fs;

use super::manifest_cmd;
use crate::{EXIT_OK, EXIT_STATE_DRIFT, EXIT_USAGE};

/// A directory that removes itself.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!(
            "o824-cli-{name}-{}-{:?}",
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
    let code = match args.first().copied() {
        Some("manifest") => manifest_cmd::run(&owned[1..], &mut out, &mut err),
        Some("keygen") => manifest_cmd::keygen(&owned[1..], &mut out, &mut err),
        Some("verify") => manifest_cmd::verify(&owned[1..], &mut out, &mut err),
        other => panic!("unknown command {other:?}"),
    };
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

fn release(scratch: &Scratch, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let target = scratch.path().join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(target, contents).expect("write");
    }
}

#[test]
fn signing_then_verifying_a_release_succeeds() {
    let scratch = Scratch::new("clean");
    release(
        &scratch,
        &[("o824.exe", "launcher"), ("docs/readme.txt", "hello")],
    );
    let key = scratch.path().join("key.hex");
    let manifest = scratch.path().join("manifest.json");

    let (code, out, _) = run(&["keygen", "--out", key.to_str().expect("path")]);
    assert_eq!(code, EXIT_OK, "{out}");
    assert!(out.contains("public key:"), "{out}");

    let (code, out, err) = run(&[
        "manifest",
        "--root",
        scratch.path().to_str().expect("path"),
        "--out",
        manifest.to_str().expect("path"),
        "--version",
        "0.1.0",
        "--key-file",
        key.to_str().expect("path"),
    ]);
    assert_eq!(code, EXIT_OK, "{out}{err}");
    assert!(out.contains("signed"), "{out}");

    let public = fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value["signature"]["publicKey"].as_str().map(str::to_owned))
        .expect("a public key in the manifest");

    let (code, out, err) = run(&[
        "verify",
        "--manifest",
        manifest.to_str().expect("path"),
        "--root",
        scratch.path().to_str().expect("path"),
        "--public-key",
        &public,
    ]);
    assert_eq!(code, EXIT_OK, "{out}{err}");
    assert!(out.contains("signature: ok"), "{out}");
    assert!(out.contains("files: ok"), "{out}");
}

#[test]
fn a_changed_file_exits_with_state_drift() {
    let scratch = Scratch::new("changed");
    release(&scratch, &[("o824.exe", "launcher")]);
    let manifest = scratch.path().join("manifest.json");
    let key = scratch.path().join("key.hex");
    let _ = run(&["keygen", "--out", key.to_str().expect("path")]);
    let _ = run(&[
        "manifest",
        "--root",
        scratch.path().to_str().expect("path"),
        "--out",
        manifest.to_str().expect("path"),
        "--key-file",
        key.to_str().expect("path"),
    ]);
    fs::write(scratch.path().join("o824.exe"), "something else").expect("tamper");

    let (code, out, _) = run(&[
        "verify",
        "--manifest",
        manifest.to_str().expect("path"),
        "--root",
        scratch.path().to_str().expect("path"),
    ]);
    assert_eq!(code, EXIT_STATE_DRIFT, "{out}");
    assert!(out.contains("changed:    o824.exe"), "{out}");
}

#[test]
fn a_missing_manifest_is_a_usage_error() {
    let scratch = Scratch::new("missing");
    let (code, _, err) = run(&[
        "verify",
        "--manifest",
        scratch.path().join("nope.json").to_str().expect("path"),
    ]);
    assert_eq!(code, 4, "a file that is not there cannot be read: {err}");
}

#[test]
fn an_unsigned_manifest_is_not_reported_as_ok() {
    let scratch = Scratch::new("unsigned");
    release(&scratch, &[("o824.exe", "launcher")]);
    let manifest = scratch.path().join("manifest.json");

    let (code, out, _) = run(&[
        "manifest",
        "--root",
        scratch.path().to_str().expect("path"),
        "--out",
        manifest.to_str().expect("path"),
    ]);
    assert_eq!(code, EXIT_OK, "{out}");
    assert!(out.contains("UNSIGNED"), "it says so out loud: {out}");

    let (code, _, err) = run(&["verify", "--manifest", manifest.to_str().expect("path")]);
    assert_eq!(code, EXIT_STATE_DRIFT, "{err}");
    assert!(err.contains("no signature"), "{err}");
}

#[test]
fn the_help_text_names_all_three_commands() {
    let (code, out, _) = run(&["manifest", "--help"]);
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("o824 manifest"));
    assert!(out.contains("o824 manifest-keygen"));
    assert!(out.contains("o824 verify-manifest"));
}

#[test]
fn a_bad_argument_is_a_usage_error() {
    let (code, _, err) = run(&["manifest", "--nonsense"]);
    assert_eq!(code, EXIT_USAGE, "{err}");
}

/// The release script needs the public half of an existing secret without minting a new pair.
/// If the two disagreed, every published manifest would be signed by a key nobody trusts.
#[test]
fn a_key_file_derives_its_own_public_key() {
    let scratch = Scratch::new("derive");
    let key = scratch.path().join("key.hex");

    let (code, out, err) = run(&["keygen", "--out", key.to_str().expect("path")]);
    assert_eq!(code, EXIT_OK, "{err}");
    let minted = out
        .lines()
        .find(|line| line.starts_with("public key: "))
        .map(|line| line["public key: ".len()..].trim().to_owned())
        .expect("a public key was printed");

    let (code, out, err) = run(&["keygen", "--from", key.to_str().expect("path")]);
    assert_eq!(code, EXIT_OK, "{err}");
    assert_eq!(out.trim(), format!("public key: {minted}"));

    // The secret itself must not leak into the derived output.
    let secret = fs::read_to_string(&key).expect("secret");
    assert!(
        !out.contains(secret.trim()),
        "the secret is printed back: {out}"
    );
}

#[test]
fn deriving_from_something_that_is_not_a_key_is_a_usage_error() {
    let (code, _, err) = run(&["keygen", "--from", "not-a-key"]);
    assert_eq!(code, EXIT_USAGE, "{err}");
}
