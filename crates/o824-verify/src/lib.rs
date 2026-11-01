//! Placeholder for the seven read-only checks that produce findings.
//!
//! Planned for milestone M1 (see `docs/MIGRATION-ROADMAP.md`). It is
//! intentionally empty: the first week builds the acceptance pipeline, not
//! business logic, and an empty crate keeps `cargo build` honest about the
//! dependency graph that the conformance tests pin down.
//!
//! This crate must never depend on `o824-plan`: a check does not know that a
//! repair exists.

#![forbid(unsafe_code)]

/// The milestone that fills this crate in.
pub const MILESTONE: &str = "M1";
