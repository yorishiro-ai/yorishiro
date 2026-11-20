//! Typed settings and static data: configuration, environment, time zone, timeouts, quotas and built-in templates.

pub mod config;
pub(crate) mod env;
pub mod http_timeouts;
pub(crate) mod quotas;
pub mod settings;
pub mod templates;
pub(crate) mod tenant_policy;
pub mod time;
