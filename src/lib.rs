pub mod app;
pub mod controllers;
pub mod data;
pub mod db;
pub mod db_enum;
pub mod dtos;

/// The composition root: the only `Hooks` implementation, and the one place that knows which editions exist.
///
/// It sits at the repository root rather than under `src/` so that everything under `src/` stays free of edition-specific wiring.
/// It is a module of this crate, not a crate of its own, so one binary carries every edition this build compiled in.
#[path = "../edition/mod.rs"]
pub mod edition;
pub mod error;
pub mod initializers;
pub mod models;
pub mod services;
pub mod tasks;
pub mod workers;

pub use edition::App;
pub use error::YorishiroError;
