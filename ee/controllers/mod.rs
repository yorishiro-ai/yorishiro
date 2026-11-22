//! Enterprise REST handlers, MCP tools and request middleware.
//! Each module mirrors a feature that only the enterprise edition serves.

// Features: one module per REST handler set, with its MCP tools in `mcp/`.
pub mod dashboard;
pub mod embedding;
pub mod entity_columns;
pub mod inference;
pub mod marketplace;
pub mod oauth;
pub mod origin;
pub mod schema_forks;
pub mod stripe;
pub mod worker_class;

// Shared by every handler.
pub mod mcp;
pub mod middleware;
