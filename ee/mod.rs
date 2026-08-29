//! The paid edition.
//!
//! Everything under this directory is licensed by `ee/LICENSE`, which adds a Competing Use
//! restriction and requires a licence key for production use. The root `LICENSE` (BUSL-1.1) covers
//! the repository excluding this directory. Both licences scope themselves by directory name, so
//! these files stay here rather than moving under `src/`: `src/lib.rs` reaches them with an explicit
//! `#[path]` instead.
//!
//! This is a module of the application crate, not a crate of its own, so one binary carries both
//! editions. What a deployment actually serves is decided at runtime: `app::licence_gate` answers
//! 404 on the gated routes until a valid licence key is configured.

pub mod controllers;
pub mod models;
pub mod services;
pub mod tasks;
