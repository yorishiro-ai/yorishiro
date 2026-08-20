//! The model layer: each module holds one subject's record shapes, its input DTOs, and the queries that read and write them.
//!
//! Shape and persistence live together, the way an Eloquent model does.
//! They were split across `models` and `repositories` until 2026-08-18, and every `repositories` module ended in `pub use crate::models::X::*` so a caller could import both halves from one path anyway.
//! That re-export is what the split cost and what the merge removes.
//! `repositories` names a pattern rather than a layer, and a directory named after it left `models` holding nothing but structs.
//!
//! What is deliberately *not* here: `migrations/` (schema versioning), `templates/*.json` (seed data), and `db.rs` (connection handling).
//! Those are the database's concerns rather than a model's.

pub mod entities;
pub mod export;
pub mod import;
pub mod maintenance;
pub mod recall;
pub mod relations;
pub mod schemas;
pub mod search;
pub mod tenancy;
