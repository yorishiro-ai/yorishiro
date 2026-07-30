# yorishiro-web

Embedded web UI for [Yorishiro](https://github.com/yotsunagi/yorishiro) — an MCP-native, multi-tenant knowledge store with user-defined schemas.

This crate uses `rust-embed` to compile the `web/` directory (a framework-free vanilla HTML/CSS/JS SPA) into the binary. It provides a `fallback_service` that serves the UI at `/` as a catch-all for paths not matched by the API routes — setup wizard, login, workspace management, and the template library.

## License

Licensed under the [Business Source License 1.1](https://github.com/yotsunagi/yorishiro/blob/master/LICENSE).
