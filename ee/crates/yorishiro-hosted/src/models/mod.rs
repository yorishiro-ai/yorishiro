//! The model layer for the tables this crate owns: record shapes and the queries that read and
//! write them, together. Ported from master's `ee/crates/yorishiro-hosted/src/models/`.
//!
//! Base's own models are reached through `yorishiro_core::models`; most modules here are the ones
//! whose tables this crate's own migrations add. `origin` is the exception: it owns no table, and
//! reads base's own `content_schemas`/`identity_templates` on `ctx.db`, since the endpoint it
//! serves is enterprise regardless of which tables it happens to read.

pub mod billing;
pub mod entity_columns;
pub mod fill_proposals;
pub mod llm_keys;
pub mod marketplace;
pub mod oauth_users;
pub mod origin;
pub mod stripe_events;
pub mod usage;
