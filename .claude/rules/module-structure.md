# Module structure

Loco's own layout, not the pre-rebuild `http/*`: controllers in `src/controllers/`, models (entity extensions) in `src/models/`, services in `src/services/`, background workers in `src/workers/`, admin/one-off commands in `src/tasks/` (via `cargo loco task`).

**MCP**: the server type (`YorishiroMcpServer`) and per-domain `#[tool_router]` implementations live in `src/services/mcp/`, since it's a thing `ee/` composes against (the sixth seam, alongside the five published contracts below) rather than route logic.
Route mounting is a one-line `src/controllers/mcp.rs::mount()`, called from `Hooks::after_routes` in `app.rs`: `rmcp`'s `StreamableHttpService` is a plain `tower::Service`, not something `Routes`/`AppRoutes` can carry, and `after_routes` is Loco's own documented hook for exactly this (custom Axum logic after Loco's own routes are built).
MCP middleware (rate limiting, request-body limits on the `/mcp` route specifically) hasn't come up yet; decide when it does, don't assume `http/middleware/` still applies.

- Use the `authorized!` / `verified!` macros for every MCP handler that needs auth.
  Do not inline the `authorize().await? + match AuthzOutcome` pattern.
- Use the `mcp_try!` macro to wrap fallible repository/service calls that should return a tool-level error on failure.
  Do not hand-roll `match call.await { Ok(x) => ..., Err(e) => Ok(err_to_tool_result(e)) }`.

## Router integration (`ee/`)

Request-ID stamping and access logging are Loco's own `request_id`/`logger` middlewares, enabled in `server.middlewares` in every `config/*.yaml`, not a hand-rolled tower layer applied at router-merge time.
`HostedApp`'s `Hooks` methods delegate to `App`'s per the composition pattern in `ee-composition.md`, so a middleware Loco applies once at boot covers both editions' routes with no merge-layer step to remember.

`Router::merge` panics on a duplicate route, and only a booted server sees it.
A route registered on both sides is therefore a startup crash rather than a compile error.

## Visibility and dead code (yorishiro-core)

- `yorishiro-core`'s only consumer outside itself is `ee/crates/yorishiro-hosted`.
  A workspace-wide grep therefore does settle whether a `pub` item is called, but it has to include `ee/`, which is a member of this workspace and the only caller of much of what this crate exports.
  The five published contracts (`build_app`, `apply_observability_layers`, `into_http_parts()`, `hex_decode`, `bearer_credential`) stay regardless: they are the seam `ee/` composes against.
  **`build_app`/`apply_observability_layers` don't exist yet** (`grep -rn "fn build_app\|apply_observability_layers" src/ ee/` finds nothing), since no router exists on this branch: this list is aspirational for those two until the controller/routing port lands.
- Keep genuinely crate-internal helpers `pub(crate)`/`pub(super)` so the distinction is visible in the code, not something a reviewer has to remember.
- `Authenticator` (`services/auth`) is meant to be a seam, not an internal detail, once every authenticated path resolves through it (`AuthContext`/`Authorized<R>`/`Verified<R>` extractors, both MCP entry points): a new authenticated entry point must resolve through that seam rather than call `authenticate` directly, or a REST route and an MCP tool could end up disagreeing about who the caller is.

## Model column lists

A SeaORM entity's `Column` enum is the one place a column list lives.
There is no `<table>_columns()` helper to write or maintain.
