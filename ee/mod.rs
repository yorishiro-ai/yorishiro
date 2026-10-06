//! The enterprise edition.
//!
//! Everything under this directory is licensed by `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.
//! The root `LICENSE` (BUSL-1.1) covers the repository excluding this directory.
//! Both licences scope themselves by directory name, so these files stay here rather than moving under `src/`.
//! `edition/mod.rs` reaches them with an explicit `#[path]`, behind the `enterprise` feature.
//!
//! This is a module of the application crate (`crate::edition::ee`), not a crate of its own, so one binary carries both editions.
//! `app.rs` is the overlay's side of the composition that `edition/` assembles.
//! What a deployment actually serves is decided at runtime: `controllers::middleware::edition::licence_gate` answers 404 on the gated routes until a valid licence key is configured.

pub(crate) mod app;
pub mod controllers;
pub mod data;
pub mod db;
pub(crate) mod dtos;
pub mod models;
pub mod services;
pub mod tasks;
pub mod workers;
