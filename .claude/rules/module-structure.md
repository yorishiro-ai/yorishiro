# Module structure

## MCP tool handler exits

**Early exits are written at the call site, not hidden in a macro.**

Authorization:

```rust
match super::authorize(...).await? {
    AuthzOutcome::Authorized(a) => a,
    AuthzOutcome::ScopeDenied(denied) => return Ok(denied),
}
```

A fallible repository call:

```rust
match call.await {
    Ok(value) => value,
    Err(err) => return Ok(err_to_tool_result(err)),
}
```

This replaces the old `authorized!` / `verified!` / `mcp_try!` macros.

**The denial is an `Ok`, not an accident.** A scope denial and a business-logic error are both successful tool results carrying `is_error: true`. Only a protocol-level failure (`ErrorData`) is an `Err`. Returning a denial as `Err` changes what clients see.

**These exits cannot become a single `?`.** `rmcp`'s `ToolRouter` fixes every tool's function type to `Result<CallToolResult, ErrorData>` (`rmcp-3.0.1`). Anything else fails to compile (`E0271`).

## Router integration

Request-ID stamping and access logging are Loco's own `request_id`/`logger` middlewares, enabled in `server.middlewares` in every `config/*.yaml`.

`Router::merge` panics on a duplicate route — a startup crash, not a compile error.

## Visibility

Everything compiles into one crate, `ee/` included. `pub` on an item says "part of this crate's external surface" — a claim worth checking. Keep genuinely internal helpers `pub(crate)`/`pub(super)`.

`Authenticator` (`services/auth`) is a seam: every authenticated path resolves through it (`AuthContext`/`Authorized<R>`/`AuditAuthorized`/`Verified<R>` extractors, `authorize`/`verify` in `services/mcp/mod.rs`). A new authenticated entry point must go through the seam, not call `authenticate` directly.

## Model column lists

The SeaORM entity's `Column` enum is the one place a column list lives. No `<table>_columns()` helper to write or maintain.
