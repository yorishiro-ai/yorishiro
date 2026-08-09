# yorishiro-server

HTTP server for [Yorishiro](https://github.com/yotsunagi/yorishiro) — an MCP-native, multi-tenant knowledge store with user-defined schemas.

This crate provides the `yorishiro-server` binary: an axum-based HTTP server that exposes `yorishiro-core`'s domain logic through both a REST API (with OpenAPI/Swagger UI) and an MCP server (Streamable HTTP). It also includes the admin CLI (`yorishiro-server admin ...`), the setup/login web UI, and the logging/configuration infrastructure.

## Not a library

While this crate does export a library API (`build_app`, `apply_observability_layers`, `logging::init`, etc.) for embedding by other binaries, it is primarily a binary crate. Most users should interact with Yorishiro through its REST API, MCP tools, or the admin CLI rather than depending on this crate directly.

## License

Licensed under the [Business Source License 1.1](https://github.com/yotsunagi/yorishiro/blob/master/LICENSE).
