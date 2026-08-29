//! The paid edition.
//!
//! Everything under this directory is licensed by `ee/LICENSE`, which adds a Competing Use
//! restriction and requires a licence key for production use. The root `LICENSE` (BUSL-1.1) covers
//! the repository excluding this directory. Both licences scope themselves by directory name, so
//! these files stay here rather than moving under `src/`: `src/lib.rs` reaches them with an explicit
//! `#[path]` instead.
//!
//! This was a separate crate (`yorishiro-hosted`) with its own `Hooks` implementation, composing on
//! top of the base one. It is one module of one crate now. Two things drove that:
//!
//! Loco's default log filter admits exactly one application crate — it chains a single
//! `Hooks::app_name()` onto a fixed module whitelist — so with two crates and the paid binary
//! booting the paid `Hooks` impl, every event from the base crate was dropped by the filter on every
//! paid-edition deployment. Naming both crates in `override_filter` worked around it; one crate
//! removes the need.
//!
//! And the guarantee the split existed to provide — paid code absent from the community binary on
//! disk — is no longer wanted. One binary carries both editions, and the licence decides what is
//! enabled at runtime rather than the artifact deciding what is present. The boundary that used to
//! be a compilation unit is now `app::licence_gate`, applied per request.

pub mod controllers;
pub mod models;
pub mod services;
pub mod tasks;
