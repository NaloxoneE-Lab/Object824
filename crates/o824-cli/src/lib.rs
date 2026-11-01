//! Subcommand parsing, exit codes, and human-facing rendering.
//!
//! Milestone M0 only owns the exit-code contract and the two flags every binary
//! must answer (`--version`, `--help`); the six subcommands land in M3, once
//! `apply` exists. Keeping the mapping here from day one is deliberate: the
//! exit codes are a machine contract that a UI switches on, so they are pinned
//! by tests before anything can depend on them.

#![forbid(unsafe_code)]

use std::fmt::Write as _;

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
pub fn exit_code(error: Option<&Error>) -> i32 {
    match error {
        None => EXIT_OK,
        Some(error) => exit_code_for(error.code),
    }
}

/// Runs one invocation. Returns the process exit code.
///
/// Until M3 this only answers `--version` and `--help`; every other invocation
/// is a usage error rather than a silent success, so a half-built kernel can
/// never look like a working one.
pub fn dispatch(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("{}", o824_version::string());
        return EXIT_OK;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") || args.is_empty() {
        print!("{}", usage());
        return EXIT_OK;
    }
    eprintln!(
        "o824: unknown command {:?} (the subcommands arrive with milestone M3)",
        args[0]
    );
    EXIT_USAGE
}

/// The usage text printed by `--help`.
pub fn usage() -> String {
    let mut text = String::new();
    let _ = writeln!(text, "o824 — the DeepSeek Harness launcher kernel");
    let _ = writeln!(text);
    let _ = writeln!(text, "usage: o824 <command> [flags]");
    let _ = writeln!(text);
    let _ = writeln!(text, "commands (milestone M3):");
    let _ = writeln!(text, "  discover   list installations on this machine");
    let _ = writeln!(text, "  doctor     diagnose one installation");
    let _ = writeln!(text, "  repair     plan or apply a repair");
    let _ = writeln!(text, "  snapshot   list, create, or restore snapshots");
    let _ = writeln!(text, "  diagnose   write a redacted diagnostic bundle");
    let _ = writeln!(text, "  mock       replay a protocol transcript");
    let _ = writeln!(text);
    let _ = writeln!(text, "flags:");
    let _ = writeln!(text, "  --version  print the build identity");
    let _ = writeln!(text, "  --help     print this text");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exit-code table is a UI contract: every row is pinned.
    #[test]
    fn exit_code_map() {
        assert_eq!(exit_code(None), 0);

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
        assert_eq!(exit_code(Some(&error)), 7);
    }

    #[test]
    fn version_and_help_are_success() {
        assert_eq!(dispatch(&["--version".to_owned()]), EXIT_OK);
        assert_eq!(dispatch(&["--help".to_owned()]), EXIT_OK);
        assert_eq!(dispatch(&[]), EXIT_OK);
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        assert_eq!(dispatch(&["frobnicate".to_owned()]), EXIT_USAGE);
    }
}
