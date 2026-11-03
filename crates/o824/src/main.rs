//! The `o824` executable: one binary, several roles.
//!
//! The kernel is a Windows command-line program that discovers DeepSeek Harness
//! installations, diagnoses why one will not start, snapshots before touching
//! anything, and can roll back. It speaks JSON-RPC over stdio to a UI and stable
//! error codes to humans.
//!
//! Everything of substance lives in the `o824-*` crates; this file only wires the
//! process up to `o824_cli::run`.

#![forbid(unsafe_code)]

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // The handles, not their locks. `Stdout::lock` is a global mutex, and holding it here
    // for the whole process deadlocks every other thread that writes: `o824 kernel` hands
    // a fresh `std::io::stdout()` to its job threads, and they block forever waiting for a
    // lock the main thread never releases. That is docs/P3-KERNEL-BUG.md — a job's events
    // never reached a real client, and the in-crate kernel tests could not see it because
    // they never go through `main`.
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = o824_cli::run(&args, &mut stdout, &mut stderr);
    let _ = stdout.flush();
    let _ = stderr.flush();

    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
