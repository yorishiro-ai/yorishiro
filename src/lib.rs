pub mod app;
pub mod controllers;
pub mod data;
pub mod db;
mod db_enum;

/// The enterprise edition.
///
/// `ee/` sits at the repository root rather than under `src/` because `ee/LICENSE` defines its own
/// Licensed Work as "everything under the `ee/` directory of this repository": the directory name is
/// what scopes that licence, so moving these files would silently change what the licence covers.
/// Compiling them into this crate does not change that scoping, since the files stay where the
/// licence points.
///
/// The enterprise edition is not a separate compilation unit. What it serves is decided at runtime by
/// `app::licence_gate`.
#[cfg(feature = "enterprise")]
#[path = "../ee/mod.rs"]
pub mod ee;
pub mod error;
pub(crate) mod initializers;
pub mod metaschema;
pub mod models;
pub mod services;
pub mod tasks;
pub mod workers;

pub use error::YorishiroError;
