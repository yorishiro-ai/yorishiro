# Editions and the `ee/` boundary

## Licences

One repository, two licences. Everything outside `ee/` is BUSL-1.1; `ee/` is the enterprise edition under `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.

## Boundary

**Only `src/lib.rs` may assemble the enterprise overlay.** Other base modules contain no `cfg(feature = "enterprise")`, no `cfg(not(feature = "enterprise"))`, no `crate::ee`, no path into `ee/`, and no overlay names, queues, branches, or registrations, comments included.
Domain dependencies run from `ee/` into the base, not back.

Crate-root assembly lives inline in `src/lib.rs`; there is no separate `edition/` directory.
The inline `edition` module preserves existing public paths.
`scripts/check_edition_boundary.py` checks all other source modules for overlay names and branches, including comments, aliases and paths split over lines.
Both check-all targets and CI run it, with deliberate-violation tests.

## Classification

Classify features by what the server does.

**The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.

Using the user's key changes who pays, not what the server does.

If the classification is unclear, ask the user which edition owns the feature.

## Composition

CE follows Loco's generated directory and file names.
When an overlay changes or extends the same functionality, its path under `ee/` mirrors the path under `src/`.
An enterprise-only feature does not require an empty counterpart in CE.
Fix the base API first, then adapt its overlay; temporary build failures during that change are not a reason to put edition branches into CE.

**`ee/` is a module of the application crate, not a package of its own.**
`src/lib.rs` declares it with `#[path = "../ee/mod.rs"]` behind the `enterprise` feature, and re-exports it through the inline `edition` module for existing callers.
The files stay at the repository root where `ee/LICENSE` scopes them while compiling into the same crate.
The root workspace lists `"."` only; there is one binary, `yorishiro`.

Loco's default log filter allows fixed modules and one `Hooks::app_name()` entry.
A second application crate needs an explicit `override_filter` entry in each `config/*.yaml`.

**There is one `Hooks` impl, inline in `src/lib.rs`, re-exported as `yorishiro::App`.**
It delegates base behaviour to `src/app.rs` and overlay contributions to `ee/app.rs`, using `RouteMounts`, `McpToolSet` and the worker registry.

The active contribution is `ee/app.rs` with the `enterprise` feature and the inline community module otherwise.
They expose identical functions, checked by both builds.
No wiring trait is needed for compile-time selection.
Runtime differences use `QueuePolicy`, `Authenticator` and `WorkerClassResolver` in `shared_store`.

`WorkerRegistry::community()` supplies the base registry, and `ee/app.rs` adds `infer-fill` through `register::<Args, Worker>()`.
Each worker declares its tags and queue through `BackgroundWorker::tags()` and `queue()`.
Loco registration, Redis queue configuration, and `yorishiro worker-tags` use the registry from `edition::workers()`; do not maintain separate lists.

The overlay calls base wiring directly, as when `ee/app.rs` uses `RouteMounts::guard_credentials` to rate-limit OAuth entry points.

A table keeps its module under `src/models/` even when only `ee/` uses it.
Every build needs an `ActiveModelBehavior` for each generated entity and a redacting `Debug` for entities with secrets.
Enterprise-specific behaviour lives in `ee/models/`.

`YorishiroError` is re-exported at the crate root (`pub use error::YorishiroError;`), so `ee/` code reaches it as `crate::YorishiroError`.

## Licence gate

`ee/controllers/middleware/edition.rs` verifies licence signatures against `ee/keys/licence-public.pem`.
Use `Routes::layer` to apply `licence_gate` only to the routes that require an active licence.

`LicenceState::is_active` compares `exp` against the current clock so enterprise access ends at expiry without a restart.

Booting with no `YORISHIRO_LICENSE_KEY` logs "no licence key configured: enterprise features are disabled" and serves every community route unchanged; booting with a valid key logs "licence key accepted: enterprise features are enabled".

CE artifacts exclude the enterprise overlay; EE artifacts compile it in and apply runtime licence gates to the routes listed below.

## Gated routes

Gated (return 404 without an active licence):
- `marketplace`
- `stripe`
- `oauth`
- `inference::gated_routes` (`infer-fill` alone)

Ungated (serve regardless):
- `embedding`
- `entity_columns`
- `origin`
- `worker_class`
- `inference`'s `/workspace/llm-provider` routes

Stripe webhooks and OAuth login require their own configuration in addition to an active licence.
The Stripe webhook verifies its signature.
Without OAuth login, users need password credentials for `POST /auth/login`.

## Licence tests

Tests live in `tests/ee/controllers/middleware/edition/`, covering verification, expiry-boundary exclusivity (`exp > now`, not `>=`), and config-file key parsing.
The suite generates its own throwaway RSA keypair per test via `openssl genrsa`/`openssl rsa -pubout` into a `tempfile::TempDir`, never a checked-in `.pem`: a committed private key reads as a leaked secret to a scanner regardless of what it actually signs, so nothing under `tests/` is a key file.

## Config file

This application resolves `config/{environment}.yaml` and reads the licence from `initializers.licence_key`.
Shipped configurations interpolate `YORISHIRO_LICENSE_KEY` into that field.
Runtime verification consumes the resolved configuration, not the environment.
