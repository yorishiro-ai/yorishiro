# yorishiro-server

HTTP server for [Yorishiro](https://github.com/yotsunagi/yorishiro) — an MCP-native, multi-tenant knowledge store with user-defined schemas.

This crate is the HTTP layer: an axum router exposing `yorishiro-core`'s domain logic through a REST API (with OpenAPI/Swagger UI) and an MCP server (Streamable HTTP), plus the admin CLI and the logging/configuration infrastructure.

## A library, not a binary

This crate ships no binary. `ee/crates/yorishiro-hosted` provides the one the product ships (`yorishiro-server`), and it composes this crate's `build_app` with the paid features and the web UI. The admin CLI lives here and is reached through that binary: `yorishiro-server admin ...`.

The dependency runs one way: this crate must not depend on `ee/`; `ee/` depends on it.

## License

Licensed under the [Business Source License 1.1](https://github.com/yotsunagi/yorishiro/blob/master/LICENSE).
