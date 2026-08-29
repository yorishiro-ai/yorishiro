# Module structure

Loco's own layout, not the pre-rebuild `http/*`: controllers in `src/controllers/`, models (entity extensions) in `src/models/`, services in `src/services/`, background workers in `src/workers/`, admin/one-off commands in `src/tasks/` (via `cargo loco task`).

**MCP**: the server type (`YorishiroMcpServer`) and per-domain `#[tool_router]` implementations live in `src/services/mcp/`, since it's a thing `ee/` composes against (the sixth seam, alongside the five published contracts below) rather than route logic.
Route mounting is a one-line `src/controllers/mcp.rs::mount()`, called from `Hooks::after_routes` in `app.rs`: `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not something `Routes`/`AppRoutes` can carry, and `after_routes` is Loco's own documented hook for exactly this (custom Axum logic after Loco's own routes are built).
MCP middleware (rate limiting, request-body limits on the `/mcp` route specifically) hasn't come up yet; decide when it does, don't assume `http/middleware/` still applies.

- **A tool handler's early exits are written at the call site, not hidden in a macro.**
  Authorization is `match super::authorize(...).await? { AuthzOutcome::Authorized(a) => a, AuthzOutcome::ScopeDenied(denied) => return Ok(denied) }`, and a fallible repository call is `match call.await { Ok(value) => value, Err(err) => return Ok(err_to_tool_result(err)) }`.
  This replaces the `authorized!` / `verified!` / `mcp_try!` macros, which read as plain assignments at their 52 call sites while each could end the function.
  The REST side expresses the same authorization as a type in the handler's signature (`Authorized<WriteScope>`), where a reader sees it without reading anything else; the MCP side cannot do that, so it spells the exit out instead.
- **The denial is an `Ok`, and that is not an accident to be tidied away.**
  A scope denial and a business-logic error are both successful tool results carrying `is_error: true`, which is what an MCP client expects; only a protocol-level failure (`ErrorData`) is an `Err`.
  Returning a denial as `Err` would change what clients see.
- **These exits cannot become a single `?`.**
  That would need the handler to return `Result<CallToolResult, SomeToolExit>`, and `rmcp`'s `ToolRouter` fixes every tool's function type to `Result<CallToolResponse, ErrorData>` (`rmcp-3.0.1`, `handler/server/router/tool.rs:202`), so anything else fails to compile inside `#[tool]`.
  Confirmed by building it: the attempt fails with `E0271`, expecting `Result<CallToolResult, ErrorData>`.
  `rmcp` does have an `IntoCallToolResult for Result<T, E>` that would map an `Err` payload to a successful `is_error: true` response, but the `#[tool]` attribute never reaches it.
  Revisit only if `rmcp` relaxes that function type.

## Router integration (`ee/`)

Request-ID stamping and access logging are Loco's own `request_id`/`logger` middlewares, enabled in `server.middlewares` in every `config/*.yaml`, not a hand-rolled tower layer applied at router-merge time.
`HostedApp`'s `Hooks` methods delegate to `App`'s per the composition pattern in `ee-composition.md`, so a middleware Loco applies once at boot covers both editions' routes with no merge-layer step to remember.

`Router::merge` panics on a duplicate route, and only a booted server sees it.
A route registered on both sides is therefore a startup crash rather than a compile error.

## Visibility and dead code

- Everything compiles into one crate, `ee/` included, so a workspace-wide grep settles whether a `pub` item is called — but it has to cover `ee/`, which is the only caller of much of what `src/` exposes.
- An item reached only from `ee/` needs no special visibility: `crate::` reaches it either way. `pub` on such an item therefore says "part of this crate's external surface", which is a claim worth checking rather than a formality.
- Keep genuinely crate-internal helpers `pub(crate)`/`pub(super)` so the distinction is visible in the code, not something a reviewer has to remember.
- `Authenticator` (`services/auth`) is meant to be a seam, not an internal detail, once every authenticated path resolves through it (`AuthContext`/`Authorized<R>`/`Verified<R>` extractors, both MCP entry points): a new authenticated entry point must resolve through that seam rather than call `authenticate` directly, or a REST route and an MCP tool could end up disagreeing about who the caller is.

## Model column lists

A SeaORM entity's `Column` enum is the one place a column list lives.
There is no `<table>_columns()` helper to write or maintain.
