//! Subcommand parsing, exit codes, and human-facing rendering.
//!
//! Milestone M1 owns the exit-code contract and the `doctor` subcommand, which
//! is what the golden files pin; the remaining subcommands land in M3, once
//! `apply` exists. Keeping the mapping here from day one is deliberate: the exit
//! codes are a machine contract that a UI switches on, so they are pinned by
//! tests before anything can depend on them.
//!
//! Everything is written to explicit writers rather than to the process's
//! streams, so a test can assert on the exact bytes a caller would see.

#![forbid(unsafe_code)]

pub mod discover;
pub mod doctor;
pub mod flags;
pub mod install;
pub mod render;
pub mod repair;
pub mod rollback;
pub mod snapshot_cmd;

use std::io::Write;

use o824_errs::{Code, Error};

/// Nothing went wrong.
pub const EXIT_OK: i32 = 0;
/// A read-only command finished with findings (the command itself succeeded).
pub const EXIT_FINDINGS: i32 = 1;
/// The command line or a path is wrong.
pub const EXIT_USAGE: i32 = 2;
/// A mutating command needs a confirmation it could not obtain.
pub const EXIT_CONFIRMATION_REQUIRED: i32 = 3;
/// A filesystem operation was refused.
pub const EXIT_PERMISSION: i32 = 4;
/// The on-disk state drifted from what the kernel recorded.
pub const EXIT_STATE_DRIFT: i32 = 5;
/// A network fetch failed.
pub const EXIT_NETWORK: i32 = 6;
/// Another process holds the kernel lock.
pub const EXIT_BUSY: i32 = 7;
/// Any other error, including one that carries no code.
pub const EXIT_INTERNAL: i32 = 8;

/// Maps a kernel error code to the process exit code.
///
/// `E_INSTALL_DIR` maps to 2, not 8: a root that is not an installation is a
/// "you named the wrong path" error, which belongs to the usage family.
pub const fn exit_code_for(code: Code) -> i32 {
    match code {
        Code::Usage | Code::InstallDir => EXIT_USAGE,
        Code::ConfirmationRequired => EXIT_CONFIRMATION_REQUIRED,
        Code::Permission => EXIT_PERMISSION,
        Code::StateDrift => EXIT_STATE_DRIFT,
        Code::Network => EXIT_NETWORK,
        Code::Busy => EXIT_BUSY,
        _ => EXIT_INTERNAL,
    }
}

/// Maps an optional kernel error to the process exit code; `None` is success.
pub fn exit_code_of(error: Option<&Error>) -> i32 {
    match error {
        None => EXIT_OK,
        Some(error) => exit_code_for(error.code),
    }
}

/// Runs one invocation. Returns the process exit code.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let Some(command) = args.first() else {
        let _ = stdout.write_all(usage().as_bytes());
        return EXIT_OK;
    };

    match command.as_str() {
        "--version" | "-V" => {
            let _ = writeln!(stdout, "{}", o824_version::string());
            EXIT_OK
        }
        "--help" | "-h" => {
            let _ = stdout.write_all(usage().as_bytes());
            EXIT_OK
        }
        "discover" => discover::run(&args[1..], stdout, stderr),
        "doctor" => doctor::run(&args[1..], stdout, stderr),
        "repair" => repair::run(&args[1..], stdout, stderr),
        "rollback" => rollback::run(&args[1..], stdout, stderr),
        "snapshot" => snapshot_cmd::run(&args[1..], stdout, stderr),
        other => {
            let _ = writeln!(
                stderr,
                "o824: unknown command {other:?} (available in this build: discover, doctor, repair, rollback, snapshot)"
            );
            EXIT_USAGE
        }
    }
}

/// The usage text printed by `--help`.
pub fn usage() -> String {
    let mut text = String::new();
    text.push_str("o824 — the DeepSeek Harness launcher kernel\n\n");
    text.push_str("usage: o824 <command> [flags]\n\n");
    text.push_str("commands:\n");
    text.push_str("  doctor     diagnose one installation\n");
    text.push('\n');
    text.push_str("commands (milestone M3):\n");
    text.push_str("  discover   list installations on this machine\n");
    text.push_str("  repair     plan or apply a repair\n");
    text.push_str("  snapshot   list, create, or restore snapshots\n");
    text.push_str("  diagnose   write a redacted diagnostic bundle\n");
    text.push_str("  mock       replay a protocol transcript\n");
    text.push('\n');
    text.push_str("flags:\n");
    text.push_str("  --version  print the build identity\n");
    text.push_str("  --help     print this text\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_capture(args: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = run(&args, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8(stdout).expect("utf8"),
            String::from_utf8(stderr).expect("utf8"),
        )
    }

    /// The exit-code table is a UI contract: every row is pinned.
    #[test]
    fn exit_code_map() {
        assert_eq!(exit_code_of(None), 0);

        assert_eq!(exit_code_for(Code::Usage), 2);
        assert_eq!(exit_code_for(Code::InstallDir), 2);
        assert_eq!(exit_code_for(Code::ConfirmationRequired), 3);
        assert_eq!(exit_code_for(Code::Permission), 4);
        assert_eq!(exit_code_for(Code::StateDrift), 5);
        assert_eq!(exit_code_for(Code::Network), 6);
        assert_eq!(exit_code_for(Code::Busy), 7);

        for code in Code::ALL {
            let mapped = exit_code_for(*code);
            if matches!(
                code,
                Code::Usage
                    | Code::InstallDir
                    | Code::ConfirmationRequired
                    | Code::Permission
                    | Code::StateDrift
                    | Code::Network
                    | Code::Busy
            ) {
                assert_ne!(mapped, 8, "{code} must not fall through to 8");
            } else {
                assert_eq!(mapped, 8, "{code} must map to 8");
            }
        }
    }

    #[test]
    fn error_carries_its_code_into_the_exit_status() {
        let error = Error::new(Code::Busy, "another o824 holds the lock");
        assert_eq!(exit_code_of(Some(&error)), 7);
    }

    #[test]
    fn version_and_help_are_success() {
        let (code, stdout, _) = run_capture(&["--version"]);
        assert_eq!(code, EXIT_OK);
        assert!(stdout.starts_with("o824 "), "{stdout}");

        let (code, stdout, _) = run_capture(&["--help"]);
        assert_eq!(code, EXIT_OK);
        assert!(stdout.contains("usage: o824 <command> [flags]"), "{stdout}");

        let (code, stdout, _) = run_capture(&[]);
        assert_eq!(code, EXIT_OK);
        assert!(stdout.contains("commands:"), "{stdout}");
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        let (code, _, stderr) = run_capture(&["frobnicate"]);
        assert_eq!(code, EXIT_USAGE);
        assert!(stderr.contains("unknown command"), "{stderr}");
    }

    #[test]
    fn the_doctor_help_is_success() {
        let (code, stdout, _) = run_capture(&["doctor", "--help"]);
        assert_eq!(code, EXIT_OK);
        assert!(stdout.contains("o824 doctor --root <path>"), "{stdout}");
    }

    #[test]
    fn doctor_without_a_root_is_a_usage_error() {
        let (code, _, stderr) = run_capture(&["doctor"]);
        assert_eq!(code, EXIT_USAGE);
        assert!(
            stderr.contains("pass --install <id> or --root <path>"),
            "{stderr}"
        );
    }

    #[test]
    fn a_typo_d_root_is_e_install_dir() {
        let (code, _, stderr) = run_capture(&["doctor", "--root", r"C:\definitely\not\here"]);
        assert_eq!(code, EXIT_USAGE);
        assert!(
            stderr.contains("is not an installation directory"),
            "{stderr}"
        );
    }
}
