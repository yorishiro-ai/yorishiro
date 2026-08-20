//! The model layer for the tables this crate owns: record shapes and the queries that read and write them, together.
//!
//! The community edition's own models are reached through `yorishiro_core::models`; these are the ones whose tables this repository's migrations add.
//!
//! This directory was named `repositories` and held nothing but a doc comment, while every query it described sat in `services`.
//! A module here owns a table, or a read across several that no single module owns.
//! A module in `services` owns a decision, and calls these when it needs one persisted.

pub mod billing;
pub mod entity_columns;
pub mod fill_proposals;
pub mod llm_keys;
pub mod origin;
pub mod stripe_events;
pub mod usage;
