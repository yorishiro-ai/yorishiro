//! The HTTP-facing layer: REST controllers, the MCP adapter (a second protocol surface over
//! the same domain logic), and the middleware both share (bearer-token auth, rate limiting).
//! Routing itself lives one level up, in `crate::routes`, which mounts `controllers::router()`
//! and `mcp::YorishiroMcpServer` onto one `axum::Router`.

pub(crate) mod controllers;
/// Public so a crate composing this one can wrap [`mcp::YorishiroMcpServer`] and serve its own
/// tools alongside these. `ToolRouter<Self>` ties a tool to the struct declaring it, so an
/// outside crate cannot add to that router; it delegates to this server instead.
pub mod mcp;
pub mod middleware;

#[cfg(test)]
#[path = "../../tests/http/mod.rs"]
mod tests;
