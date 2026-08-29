pub mod app;
pub mod controllers;
pub mod db;
/// The paid edition.
///
/// `ee/` sits at the repository root rather than under `src/` because `ee/LICENSE` defines its
/// own Licensed Work as "everything under the `ee/` directory of this repository": the directory
/// name is what scopes that licence, so moving these files would silently change what the licence
/// covers. Compiling them into this crate does not change that scoping, since the files stay where
/// the licence points.
///
/// Being one crate rather than two is what lets loco's default log filter admit this application
/// at all (`Hooks::app_name()` contributes exactly one entry), and the licence boundary it used to
/// carry now lives in `app::licence_gate` as a per-request check.
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
