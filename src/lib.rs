/// Migration schema and migration runner (see `migration/src/lib.rs`).
#[path = "../migration/src/lib.rs"]
pub mod migration;

pub mod app;
pub mod controllers;
pub mod db;
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
#[path = "../ee/mod.rs"]
pub mod ee;
pub mod error;
pub mod metaschema;
pub mod models;
pub mod services;
pub mod tasks;
pub mod templates;
pub mod workers;

pub use error::YorishiroError;
