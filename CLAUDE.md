# Rust coding rules for yorishiro

## Error handling

- Use `yorishiro_core::ResultExt` (`.internal()`) for any fallible call that
  produces a non-`YorishiroError` error. Never write
  `map_err(|e| YorishiroError::Internal(e.into()))` by hand.
  `.internal()` only converts an existing error (`E: Into<anyhow::Error>`) and
  cannot attach a message, so it does not cover raising an `Internal` from a
  formatted string with no source error. `services/embedding/onnx.rs` has a
  private `fn internal(message: impl Display)` for exactly that case — a local
  helper like it is the sanctioned pattern when a module needs it repeatedly.
  Do not promote one to a shared API until a second module actually wants it.
- Use `YorishiroError::not_found(msg)` for NotFound construction instead of
  building the struct literal directly.
- The `into_response` mapping from `YorishiroError` to HTTP status+body lives in
  `YorishiroError::into_http_parts()` (in `yorishiro_core::error`). `ApiError`
  calls it, and so must any other axum error wrapper built on `YorishiroError`
  — never duplicate the match block.

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

## Visibility and dead code (yorishiro-core)

- `yorishiro-core`'s `pub` API has consumers outside this repository. **"No caller
  in this workspace" does not mean unused** -- a repo-wide grep can only prove
  that *this* repo doesn't call it. Check the downstream consumers before
  deleting a `pub` item or narrowing its visibility.
- Keep genuinely crate-internal helpers `pub(crate)`/`pub(super)` so the
  distinction is visible in the code, not something a reviewer has to remember.

## Module structure

- Controllers go in `http/controllers/`, middleware in `http/middleware/`,
  MCP tools in `http/mcp/`, services in `services/`.

## Tests

- Tests live at the crate root, in `tests/`, as flat integration test files
  (e.g. `crates/yorishiro-core/tests/repositories_schemas.rs`), never inline
  in `src/` behind `#[cfg(test)]` and never in a `src/**/tests/` or
  `src/**/tests.rs` module. Each file under `tests/` compiles as its own
  integration-test binary against the crate's public API.
- **Name a test file after the `src/` module path it covers, with `/` replaced
  by `_`** — `src/services/auth.rs` is tested by `tests/services_auth.rs`,
  `src/http/controllers/schemas.rs` by `tests/http_controllers_schemas.rs`.
  The rule used to say "module/feature", and that "or feature" was enough
  slack for `yorishiro-server` to end up with `schemas.rs` and
  `http_middleware_auth.rs` side by side — two names for the same depth of
  thing, so neither told you where to look. The mapping is now mechanical:
  read the filename, you know the module; know the module, you know the
  filename. A test genuinely spanning several modules is named for the
  behaviour it covers (`http_routes_layers.rs`), not for one arbitrary member.
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

- The newtype wrapper over `YorishiroError` for axum is `ApiError`
  (`yorishiro-server`). The name is fixed — do not rename.
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
- Tag format: `v{version}` (e.g. `v0.8.1`). Releases are cut by running the
  `Release` workflow (`workflow_dispatch` with a `version` input) from the
  Actions tab or `gh workflow run release.yml -f version=X.Y.Z` -- it bumps
  `Cargo.toml`/`Cargo.lock`, commits, and creates the tag itself. Do not
  hand-edit the version or create the tag locally.
