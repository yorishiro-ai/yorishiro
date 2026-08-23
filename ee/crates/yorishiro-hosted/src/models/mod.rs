//! The model layer for the tables this crate owns: record shapes and the queries that read and
//! write them, together. Ported from master's `ee/crates/yorishiro-hosted/src/models/`.
//!
//! Base's own models are reached through `yorishiro_core::models`; these are the ones whose
//! tables this crate's own migrations add.

pub mod billing;
pub mod usage;
