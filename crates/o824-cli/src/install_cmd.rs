//! `o824 install` and `o824 uninstall`.
//!
//! The launcher is not an installer package: it is two executables that want a directory of
//! their own. This command gives them one, per user, without asking for administrator rights
//! and without inventing a format nobody else can read:
//!
//! * `%LOCALAPPDATA%\o824` — a location whose files are **not** carrying a low-integrity label,
//!   which is what makes the notification-area icon work and keeps SmartScreen to the first
//!   launch instead of every launch from a build tree;
//! * one Start Menu shortcut, because that is how people launch things;
//! * one per-user uninstall entry, so it can be removed from Settings like anything else.
//!
//! `uninstall` reverses all three. Deleting a program is destructive, so it demands `--yes`
//! (§20.7: a destructive action gets a second confirmation).

use std::io::Write;
use std::path::{Path, PathBuf};

use o824_errs::{Code, Error};

use crate::flags::{FlagSet, FlagSpec};
use crate::{exit_code_for, EXIT_CONFIRMATION_REQUIRED, EXIT_OK};

const USAGE: &str = "\
usage: o824 install [--dir <path>] [--from <path>] [--no-shortcut]
       o824 uninstall [--dir <path>] --yes

  install    copy the launcher into a directory of its own, add a Start Menu shortcut, and
             register an uninstall entry for this user
  uninstall  remove all three; refuses without --yes

Default location: %LOCALAPPDATA%\\o824. No administrator rights are needed or requested.
";

/// The two files an installation consists of.
const FILES: [&str; 2] = o824_install::FILES;

/// `o824 install`.
pub fn install(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![
        FlagSpec::valued("dir"),
        FlagSpec::valued("from"),
        FlagSpec::boolean("no-shortcut"),
    ]);
    if let Err(error) = flags.parse("install", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    let source = match flags.value("from") {
        Some(path) => PathBuf::from(path),
        None => match std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            Some(directory) => directory,
            None => {
                let error = Error::new(
                    Code::Internal,
                    "install: cannot tell where this program lives",
                );
                let _ = writeln!(stderr, "{error}");
                return exit_code_for(error.code);
            }
        },
    };
    let target = match flags.value("dir") {
        Some(path) => PathBuf::from(path),
        None => default_directory(),
    };

    // Both files must be present: an installation with a kernel but no window is not an
    // installation anyone asked for.
    for name in FILES {
        if !source.join(name).is_file() {
            let error = Error::new(
                Code::Usage,
                format!(
                    "install: {name} is not in {:?}. Point --from at the folder that contains o824.exe and o824-gui.exe",
                    source.display().to_string()
                ),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    }

    if let Err(error) = std::fs::create_dir_all(&target) {
        let error = Error::new(
            Code::Permission,
            format!(
                "install: cannot create {:?}: {error}",
                target.display().to_string()
            ),
        );
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }

    for name in FILES {
        let from = source.join(name);
        let to = target.join(name);
        // Copying a file onto itself truncates it on Windows; the running program may well be
        // the one being installed.
        if same_file(&from, &to) {
            continue;
        }
        if let Err(error) = std::fs::copy(&from, &to) {
            let error = Error::new(
                Code::Permission,
                format!(
                    "install: cannot copy {name} to {:?}: {error}",
                    target.display().to_string()
                ),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
    }

    let shortcut = if flags.flag("no-shortcut") {
        None
    } else {
        make_shortcut(&target.join(FILES[1])).ok()
    };

    match write_uninstall_entry(&target) {
        Ok(key) => {
            let _ = writeln!(stdout, "registered for removal: {key}");
        }
        Err(error) => {
            // Not fatal: the files are where they belong, which is the part that matters.
            let _ = writeln!(stderr, "warning: {error}");
        }
    }

    let _ = writeln!(stdout, "installed to {}", target.display());
    match shortcut {
        Some(path) => {
            let _ = writeln!(stdout, "start menu shortcut: {}", path.display());
        }
        None => {
            let _ = writeln!(stdout, "no Start Menu shortcut (--no-shortcut)");
        }
    }
    let _ = writeln!(
        stdout,
        "run it from there, not from the build folder: a build folder inside a sandbox marks\n\
         its files low-integrity, and Windows then refuses the notification-area icon."
    );
    let _ = writeln!(
        stdout,
        "this build is unsigned, so SmartScreen will ask once; it is about identity, not damage.\n\
         remove it later with: o824 uninstall --yes"
    );
    EXIT_OK
}

/// `o824 uninstall`.
pub fn uninstall(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        let _ = stdout.write_all(USAGE.as_bytes());
        return EXIT_OK;
    }

    let mut flags = FlagSet::new(vec![FlagSpec::valued("dir"), FlagSpec::boolean("yes")]);
    if let Err(error) = flags.parse("uninstall", args) {
        let _ = writeln!(stderr, "{error}");
        return exit_code_for(error.code);
    }
    if !flags.flag("yes") {
        let error = Error::new(
            Code::ConfirmationRequired,
            "uninstall: this deletes the program; pass --yes if that is what you want",
        );
        let _ = writeln!(stderr, "{error}");
        return EXIT_CONFIRMATION_REQUIRED;
    }

    let target = match flags.value("dir") {
        Some(path) => PathBuf::from(path),
        None => default_directory(),
    };

    remove_uninstall_entry();
    if let Some(path) = shortcut_path() {
        let _ = std::fs::remove_file(&path);
    }

    if target.exists() {
        if let Err(error) = std::fs::remove_dir_all(&target) {
            let error = Error::new(
                Code::Permission,
                format!(
                    "uninstall: cannot remove {:?}: {error}",
                    target.display().to_string()
                ),
            );
            let _ = writeln!(stderr, "{error}");
            return exit_code_for(error.code);
        }
        let _ = writeln!(stdout, "removed {}", target.display());
    } else {
        let _ = writeln!(stdout, "nothing at {}", target.display());
    }
    let _ = writeln!(
        stdout,
        "removed the Start Menu shortcut and the removal entry"
    );
    EXIT_OK
}

/// `%LOCALAPPDATA%\o824`, via the install crate.
pub fn default_directory() -> PathBuf {
    o824_install::default_directory()
}

/// `%APPDATA%\...\O824.lnk`.
pub fn shortcut_path() -> Option<PathBuf> {
    o824_install::shortcut_path()
}

/// The values a per-user uninstall entry carries.
pub fn uninstall_values(install_dir: &Path) -> Vec<(&'static str, String)> {
    o824_install::uninstall_values(install_dir, o824_version::VERSION)
}

/// Where the uninstall entry lives.
pub const UNINSTALL_KEY: &str = o824_install::UNINSTALL_KEY;

/// True when both paths name the same file on disk.
fn same_file(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Writes the removal entry; the registry itself lives in `o824-install`.
fn write_uninstall_entry(install_dir: &Path) -> Result<String, Error> {
    o824_install::write_uninstall_entry(install_dir, o824_version::VERSION)
}

/// Removes the removal entry.
fn remove_uninstall_entry() {
    o824_install::remove_uninstall_entry()
}

/// Creates the Start Menu shortcut.
fn make_shortcut(target: &Path) -> Result<PathBuf, Error> {
    o824_install::make_shortcut(target)
}
