# Rust coding rules for yorishiro

## Editions and the `ee/` boundary

- One repository, two licences.
  Everything outside `ee/` is BUSL-1.1; `ee/` is the paid edition under `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.
- **`crates/yorishiro-{core,server}` must not depend on `ee/`.
  `ee/` depends on them.**
  One binary composes both.
  A `use` or a path dependency pointing from `crates/` into `ee/` inverts this and is the one import direction that is always wrong.
- Which side a feature belongs on is decided by what the feature *is*, never by what it needs.
  **The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.
  "The user brings their own key" does not move a feature out of `ee/`, because it changes who pays rather than what the server does.
  Any sentence of the form "it does not depend on X" is the wrong test.
- Unclear cases are a question for the user, asked as the classification itself rather than buried in the options of an implementation question.

## Error handling

- Use `yorishiro_core::ResultExt` (`.internal()`) for any fallible call that produces a non-`YorishiroError` error.
  Never write `map_err(|e| YorishiroError::Internal(e.into()))` by hand.
  `.internal()` only converts an existing error (`E: Into<anyhow::Error>`) and cannot attach a message, so it does not cover raising an `Internal` from a formatted string with no source error.
  `services/embedding/onnx.rs` has a private `fn internal(message: impl Display)` for exactly that case — a local helper like it is the sanctioned pattern when a module needs it repeatedly.
  Do not promote one to a shared API until a second module actually wants it.
- Use `YorishiroError::not_found(msg)` for NotFound construction instead of building the struct literal directly.
- The `into_response` mapping from `YorishiroError` to HTTP status+body lives in `YorishiroError::into_http_parts()` (in `yorishiro_core::error`).
  `ApiError` calls it, and so must any other axum error wrapper built on `YorishiroError` — never duplicate the match block.
  `ee/`'s `HostedApiError` delegates to it for the same reason.
  Both names are fixed; do not rename either.
- The Stripe webhook (`stripe_webhook`) returns a plain `impl IntoResponse` with raw status codes, because Stripe expects simple text rather than a JSON error envelope.
  It is the sole exception to using `HostedApiError`.

## Router integration (`ee/`)

- The hosted router MUST have `apply_observability_layers()` applied before it is merged into `build_app()`.
  `axum::Router::merge` does not propagate layers.
- `Router::merge` panics on a duplicate route, and only a booted server sees it.
  A route registered on both sides is therefore a startup crash rather than a compile error.

## MCP handlers (yorishiro-server)

- Use the `authorized!` / `verified!` macros for every MCP handler that needs auth.
  Do not inline the `authorize().await? + match AuthzOutcome` pattern.
- Use the `mcp_try!` macro to wrap fallible repository/service calls that should return a tool-level error on failure.
  Do not hand-roll `match call.await { Ok(x) => ..., Err(e) => Ok(err_to_tool_result(e)) }`.

## Repository column lists (yorishiro-core)

- When a repository queries/returns/inserts the same set of columns in multiple places, extract a `fn <table>_columns() -> [<Iden>; N]` helper (see `schema_columns()` in `repositories/schemas/mod.rs` for the pattern).
  All `.columns(...)` call sites use this helper.
  Adding a column means updating one place.

## Visibility and dead code (yorishiro-core)

- `yorishiro-core`'s consumers are all in this workspace now: `yorishiro-server` and `ee/crates/yorishiro-hosted`.
  A workspace-wide grep therefore does settle whether a `pub` item is called -- but it has to include `ee/`, which is a member of this workspace and the only caller of much of what core exports.
  The five published contracts (`build_app`, `apply_observability_layers`, `into_http_parts()`, `hex_decode`, `bearer_credential`) stay regardless: they are the seam `ee/` composes against.
- Keep genuinely crate-internal helpers `pub(crate)`/`pub(super)` so the distinction is visible in the code, not something a reviewer has to remember.
- `Authenticator` (`services/auth`) is a seam, not an internal detail.
  Every authenticated path -- the `AuthContext`/`Authorized<R>`/`Verified<R>` extractors and both MCP entry points -- resolves through the one `AppState::authenticator`.
  **A new authenticated entry point must resolve through it too**: one that calls `authenticate` directly would keep this crate's rule while every other path honours a replacement, so a REST route and an MCP tool would disagree about who the caller is.

## Module structure

- Controllers go in `http/controllers/`, middleware in `http/middleware/`, MCP tools in `http/mcp/`, services in `services/`.

## Tests

- `tests/` mirrors `src/` exactly: same directories, same filenames.
  The test body lives in `tests/`, and the `src` module it covers ends with a bridge:

  ```rust
  #[cfg(test)]
  #[path = "../../tests/repositories/schemas/mod.rs"]
  mod tests;
  ```

  so it compiles as that module's own `mod tests` rather than as an external integration test.
  Never inline a test body in `src/`.
  Two consequences follow, and both matter:
  - **Private items are testable.** `pub(crate)` and private functions are reachable, so a test never needs visibility widened for its own sake.
  - **`autotests = false` is required** in `Cargo.toml`.
    Without it cargo also compiles each `tests/*.rs` as a standalone integration target, where `use crate::` fails.
    This is also why the layout cannot be adopted one file at a time — all three crates are already on it, in ~77 places.
- Test-only fixtures live in a `#[cfg(test)]`, `pub(crate)` `test_support` module (`crates/yorishiro-core/src/lib.rs`).
  `tests/` reaches it as `crate::test_support`.
  Do **not** widen it to `pub` or drop the `#[cfg(test)]`: under the bridge neither is needed, and `pub` would put fixtures on the crate's public surface for no reader outside these tests.
- A `src` file with nothing to test (a `mod`-only re-export hub) gets no test file and no bridge.
- In `ee/`, shared helpers (`tests/test_helpers.rs`) are declared **once**, in `tests/lib.rs`, and reached elsewhere with `use crate::tests::test_helpers;`.
  Declaring `mod test_helpers;` in several files compiles it several times and trips `clippy::duplicate_mod`.

## Imports

- Always `use axum::http::StatusCode;` — never use the fully-qualified `axum::http::StatusCode` inline in function signatures or bodies.
- Group imports: std → external crates → workspace crates → crate-internal.
  `cargo fmt` handles ordering within groups.

## Naming

- The newtype wrapper over `YorishiroError` for axum is `ApiError` (`yorishiro-server`).
  The name is fixed — do not rename.
- Avoid naming collisions across layers.
  If a type name already exists in `yorishiro-core`, the server-layer type that wraps/extends it should have a distinct name (e.g. core's `AuthContext` vs. server's auth extractor).

## Git workflow

- **Never push directly to master.**
  All changes go through a PR.
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
- Every PR that adds/changes config must update `config.example.yml` and `docs/configuration.md` (English + Japanese).

## Versioning

- `workspace.package.version` in the root `Cargo.toml` is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Tag format: `v{version}` (e.g. `v0.8.1`).
  Releases are cut by running the `Release` workflow (`workflow_dispatch` with a `version` input) from the Actions tab or `gh workflow run release.yml -f version=X.Y.Z` -- it bumps `Cargo.toml`/`Cargo.lock`, commits, and creates the tag itself.
  Do not hand-edit the version or create the tag locally.
