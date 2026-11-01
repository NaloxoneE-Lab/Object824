//! The `o824` executable: one binary, several roles.
//!
//! The kernel is a Windows command-line program that discovers DeepSeek
//! Harness installations, diagnoses why one will not start, snapshots before
//! touching anything, and can roll back. It speaks JSON-RPC over stdio to a UI
//! and stable error codes to humans.
//!
//! Everything of substance lives in the `o824-*` crates; this file only wires
//! the process up to `o824_cli::dispatch`.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(u8::try_from(o824_cli::dispatch(&args)).unwrap_or(1))
}
