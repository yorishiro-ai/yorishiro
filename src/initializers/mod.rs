//! Process-lifetime wiring that Loco runs at boot: logging, database, authentication, queue, embedding and rate limiting.

pub(crate) mod auth;
pub(crate) mod database;
pub mod db_load_guard;
pub(crate) mod embedding;
pub mod logger;
pub(crate) mod queue;
pub(crate) mod rate_limit;
