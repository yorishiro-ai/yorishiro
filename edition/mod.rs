//! The composition root: which editions this build carries, and how they are assembled into one Loco application.
//!
//! This is assembly only.
//! Behaviour lives in `src/` (the community base) and in `ee/` (the enterprise overlay), and neither of them names the other: `src/` never refers to this module's contents, and `ee/` reaches `src/` the same way any other caller does.
//! This module sits outside `src/` so that nothing under `src/` has to know an overlay exists.
//!
//! [`App`] is the crate's only `Hooks` implementation.
//! Each hook calls the matching function in [`crate::app`] for the base behaviour, then lets the active edition add to it through `active`.
//!
//! `active` is chosen at compile time: `ee/app.rs` when the `enterprise` feature is on, `community.rs` otherwise.
//! The two expose the same function signatures, so a mismatch fails the build of whichever edition is behind.
//! They are not behind a trait because there is one overlay and no runtime choice to make.

mod app;
#[cfg(not(feature = "enterprise"))]
mod community;

#[cfg(feature = "enterprise")]
#[path = "../ee/mod.rs"]
pub mod ee;

#[cfg(feature = "enterprise")]
use ee::app as active;

#[cfg(not(feature = "enterprise"))]
use community as active;

pub use app::{App, worker_tags, workers};
