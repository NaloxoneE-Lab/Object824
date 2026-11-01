//! Placeholder for the executor: snapshot first, then journal, then steps, then
//! rollback on failure.
//!
//! Planned for milestone M3 (see `docs/MIGRATION-ROADMAP.md`). It is
//! intentionally empty: the first week builds the acceptance pipeline, not
//! business logic, and an empty crate keeps `cargo build` honest about the
//! dependency graph that the conformance tests pin down.
//!
//! This crate must never depend on `o824-cli`: the kernel does not know that
//! human-facing output exists.

#![forbid(unsafe_code)]

/// The milestone that fills this crate in.
pub const MILESTONE: &str = "M3";
