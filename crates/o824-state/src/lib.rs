//! Placeholder for atomic state files: write to a sibling, then rename.
//!
//! Planned for milestone M1 (see `docs/MIGRATION-ROADMAP.md`). It is
//! intentionally empty: the first week builds the acceptance pipeline, not
//! business logic, and an empty crate keeps `cargo build` honest about the
//! dependency graph that the conformance tests pin down.

#![forbid(unsafe_code)]

/// The milestone that fills this crate in.
pub const MILESTONE: &str = "M1";
