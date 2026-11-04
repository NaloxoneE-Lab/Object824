//! `o824 install` and `o824 uninstall`, as a caller sees them.
//!
//! Only the decisions are tested here: which arguments are refused, and in which order things
//! happen. A test that ran `uninstall --yes` would delete the removal entry of whoever is
//! running the suite, which is not something a test gets to do.

use std::fs;
use std::path::Path;

use super::install_cmd;
use crate::{EXIT_CONFIRMATION_REQUIRED, EXIT_OK, EXIT_USAGE};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!(
            "o824-install-{name}-{}-{:?}",
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

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_install(args: &[&str]) -> (i32, String, String) {
    let owned: Vec<String> = args.iter().map(|argument| (*argument).to_owned()).collect();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let code = install_cmd::install(&owned, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

fn run_uninstall(args: &[&str]) -> (i32, String, String) {
    let owned: Vec<String> = args.iter().map(|argument| (*argument).to_owned()).collect();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let code = install_cmd::uninstall(&owned, &mut out, &mut err);
    (
        code,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

#[test]
fn installing_without_the_two_programs_is_a_usage_error() {
    let scratch = Scratch::new("empty");
    let (code, _, err) = run_install(&[
        "--from",
        scratch.path().to_str().expect("path"),
        "--dir",
        scratch.path().join("dest").to_str().expect("path"),
    ]);
    assert_eq!(code, EXIT_USAGE, "{err}");
    // The message names whichever program is missing first, and where it looked.
    assert!(err.contains("is not in"), "{err}");
    assert!(
        err.contains("o824.exe") || err.contains("o824-gui.exe"),
        "{err}"
    );
    // Nothing was created, because the check happens before the first write.
    assert!(
        !scratch.path().join("dest").exists(),
        "a failed install left a directory behind"
    );
}

#[test]
fn a_directory_with_both_programs_installs() {
    let scratch = Scratch::new("both");
    let source = scratch.path().join("src");
    fs::create_dir_all(&source).expect("source");
    for name in o824_install::FILES {
        fs::write(source.join(name), format!("pretend {name}")).expect("write");
    }
    let dest = scratch.path().join("dest");

    let (code, out, err) = run_install(&[
        "--from",
        source.to_str().expect("path"),
        "--dir",
        dest.to_str().expect("path"),
        "--no-shortcut",
    ]);
    assert_eq!(code, EXIT_OK, "{out}{err}");
    for name in o824_install::FILES {
        assert!(dest.join(name).is_file(), "{name} was not installed");
    }
    // The window is what people launch, so the message has to say where it went.
    assert!(out.contains("installed to"), "{out}");
    assert!(
        out.contains("remove it later with: o824 uninstall --yes"),
        "{out}"
    );
}

#[test]
fn uninstalling_without_confirmation_is_refused() {
    let scratch = Scratch::new("refuse");
    let (code, _, err) = run_uninstall(&["--dir", scratch.path().to_str().expect("path")]);
    assert_eq!(code, EXIT_CONFIRMATION_REQUIRED, "{err}");
    assert!(err.contains("--yes"), "{err}");
}

#[test]
fn the_help_text_names_both_commands() {
    let (code, out, _) = run_install(&["--help"]);
    assert_eq!(code, EXIT_OK);
    assert!(out.contains("o824 install"));
    assert!(out.contains("o824 uninstall"));
    assert!(
        out.contains("%LOCALAPPDATA%"),
        "the default location is stated: {out}"
    );
}

#[test]
fn the_uninstall_entry_names_this_installation() {
    let directory = Path::new(r"C:\Users\someone\AppData\Local\o824");
    let values = install_cmd::uninstall_values(directory);
    let uninstall = values
        .iter()
        .find(|(key, _)| *key == "UninstallString")
        .map(|(_, value)| value.clone())
        .expect("an uninstall string");
    assert!(
        uninstall.starts_with('"'),
        "a path with spaces must be quoted: {uninstall}"
    );
    assert!(uninstall.ends_with("uninstall --yes"), "{uninstall}");
}
