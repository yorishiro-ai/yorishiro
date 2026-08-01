# Rust coding rules for yorishiro

## Error handling

- Use `yorishiro_core::ResultExt` (`.internal()`) for any fallible call that
  produces a non-`YorishiroError` error. Never write
  `map_err(|e| YorishiroError::Internal(e.into()))` by hand.
- Use `YorishiroError::not_found(msg)` for NotFound construction instead of
  building the struct literal directly.
- The `into_response` mapping from `YorishiroError` to HTTP status+body lives in
  `YorishiroError::into_http_parts()` (in `yorishiro_core::error`). Both
  `ApiError` (server) and `HostedApiError` (hosted) call it — never duplicate
  the match block.

## MCP handlers (yorishiro-server)

- Use the `authorized!` / `verified!` macros for every MCP handler that needs
  auth. Do not inline the `authorize().await? + match AuthzOutcome` pattern.
- Use the `mcp_try!` macro to wrap fallible repository/service calls that should
  return a tool-level error on failure. Do not hand-roll
  `match call.await { Ok(x) => ..., Err(e) => Ok(err_to_tool_result(e)) }`.

## Repository column lists (yorishiro-core)

- When a repository queries/returns/inserts the same set of columns in multiple
  places, extract a `fn <table>_columns() -> [<Iden>; N]` helper (see
  `schema_columns()` in `repositories/schemas/mod.rs` for the pattern).
  All `.columns(...)` call sites use this helper. Adding a column means updating
  one place.

## Module structure

- Controllers go in `http/controllers/`, middleware in `http/middleware/`,
  MCP tools in `http/mcp/`, services in `services/`.

## Tests

- Tests live at the crate root, in `tests/`, as flat integration test files
  (e.g. `crates/yorishiro-core/tests/repositories_schemas.rs`), never inline
  in `src/` behind `#[cfg(test)]` and never in a `src/**/tests/` or
  `src/**/tests.rs` module. Each file under `tests/` compiles as its own
  integration-test binary against the crate's public API — name it after the
  module/feature it covers (e.g. `services_auth.rs`, `http_middleware_auth.rs`).
- Because `tests/` compiles the crate as an ordinary external dependency
  (no `cfg(test)`), test-only fixtures/helpers the crate wants to expose live
  in a `pub` (not `pub(crate)`), `#[doc(hidden)]` module such as
  `yorishiro_core::test_support` — see `crates/yorishiro-core/src/lib.rs` and
  `crates/yorishiro-server/src/lib.rs`. Do not gate these helper modules with
  `#[cfg(test)]`; that would make them invisible to `tests/`.
- Do not add `exclude = ["src/**/tests/", ...]` to a crate's `Cargo.toml` —
  once tests are migrated out of `src/`, no such exclude is needed.

## Imports

- Always `use axum::http::StatusCode;` — never use the fully-qualified
  `axum::http::StatusCode` inline in function signatures or bodies.
- Group imports: std → external crates → workspace crates → crate-internal.
  `cargo fmt` handles ordering within groups.

## Naming

- Newtype wrappers over `YorishiroError` for axum: `ApiError` (server),
  `HostedApiError` (hosted). The names are fixed — do not rename.
- Avoid naming collisions across layers. If a type name already exists in
  `yorishiro-core`, the server-layer type that wraps/extends it should have a
  distinct name (e.g. core's `AuthContext` vs. server's auth extractor).

## Git workflow

- **Never push directly to master.** All changes go through a PR.
- Branch naming: `feat/<name>`, `fix/<name>`, `docs/<name>`, `refactor/<name>`
- **Before creating a PR branch**, always:
  1. `git fetch origin master`
  2. `git checkout master && git pull origin master`
  3. `git checkout -b <branch-name>` (from up-to-date master)
- **Before pushing a PR branch**, always:
  1. Run `cargo check`, `cargo clippy -- -D warnings`, `cargo fmt --check` locally
  2. Confirm all pass before pushing
- **Before merging a PR**, always:
  1. Verify all CI checks have passed on the latest commit
  2. If the branch is behind master, rebase first: `git fetch origin master && git rebase origin/master`
- Every PR must pass CI (check + security) before merge.
- Squash merge is the default merge strategy.
- Every PR that changes source code must also update docs (English + Japanese).
  The `doc-check` workflow warns automatically if this is missing.
- Every PR that adds/changes config must update `.env.example`,
  `config.example.yml`, and `docs/configuration.md` (English + Japanese).

## Versioning

- `workspace.package.version` in the root `Cargo.toml` is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Tag format: `v{version}` (e.g. `v0.8.1`). Tag every release commit.
