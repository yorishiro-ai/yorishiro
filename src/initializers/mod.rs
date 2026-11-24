//! Process-lifetime wiring: database pools, authentication, queue relay, embedding provider, rate limiting and the connection-load guard.

pub(crate) mod auth;
pub(crate) mod database;
pub mod db_load_guard;
pub(crate) mod embedding;
pub(crate) mod queue;
pub(crate) mod rate_limit;
