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

Which side a feature belongs on is decided by what the feature *is*, never by what it needs.

**The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.

"The user brings their own key" does not move a feature out of `ee/`, because it changes who pays rather than what the server does. Any sentence of the form "it does not depend on X" is the wrong test.

Unclear cases are a question for the user, asked as the classification itself rather than buried in the options of an implementation question.

## Composition

CE follows Loco's generated directory and file names.
When an overlay changes or extends the same functionality, its path under `ee/` mirrors the path under `src/`.
An enterprise-only feature does not require an empty counterpart in CE.
Fix the base API first, then adapt its overlay; temporary build failures during that change are not a reason to put edition branches into CE.

**`ee/` is a module of the application crate, not a package of its own.**
`src/lib.rs` declares it with `#[path = "../ee/mod.rs"]` behind the `enterprise` feature, and re-exports it through the inline `edition` module for existing callers.
The files stay at the repository root where `ee/LICENSE` scopes them while compiling into the same crate.
The root workspace lists `"."` only; there is one binary, `yorishiro`.

One crate is also what lets loco's own logging reach this application at all: its default filter is a fixed module whitelist plus exactly one `Hooks::app_name()` entry (`logger.rs:192-210`), so a second application crate would be silent unless every `config/*.yaml` named it in `override_filter`.

**There is one `Hooks` impl, inline in `src/lib.rs`, re-exported as `yorishiro::App`.**
It delegates base behaviour to `src/app.rs` and overlay contributions to `ee/app.rs`, using `RouteMounts`, `McpToolSet` and the worker registry.

The active contribution is `ee/app.rs` with the `enterprise` feature and the inline community module otherwise.
They expose identical functions, checked by both builds.
No wiring trait is needed for compile-time selection.
Runtime differences use `QueuePolicy`, `Authenticator` and `WorkerClassResolver` in `shared_store`.

**Workers have one registration per edition.** `src/workers/registry.rs` owns `WorkerRegistry`, a plain value whose `community()` is the baseline and whose `register::<Args, Worker>()` is how `ee/app.rs` adds `infer-fill`. A worker's tags and queue are whatever its `BackgroundWorker::tags()` and `queue()` return. `edition::workers()` composes the final registry, and Loco registration, the Redis queue configuration, and `yorishiro worker-tags` all derive from it. Do not keep a second list of tags or queues anywhere.

**The overlay's paths reach the base by naming, not by the base knowing them.** For example `ee/app.rs` calls `RouteMounts::guard_credentials` for its OAuth entry points, so the per-IP rate limit in `src/` applies to them without `src/` listing any overlay path.

**A table keeps its module under `src/models/` even when only `ee/` uses it.** Migrations and generated entities are one shared schema, and every generated entity needs an `ActiveModelBehavior` (and, for secret-bearing ones, a redacting `Debug`) in every build. The behaviour `ee/` owns for that table lives in `ee/models/`.

`YorishiroError` is re-exported at the crate root (`pub use error::YorishiroError;`), so `ee/` code reaches it as `crate::YorishiroError`.

## Licence gate

**The licence gate is a per-request layer, not a compilation boundary.** `ee/controllers/middleware/edition.rs` verifies a signed licence key against `ee/keys/licence-public.pem`. Its `licence_gate` reads that state on every request and answers 404 on the routes it is attached to, applied through `Routes::layer` so it reaches exactly those routes and cannot leak onto the community ones.

Per request rather than at boot is deliberate: `LicenceState::is_active` compares `exp` against the current clock, so a key that lapses while the process runs stops unlocking enterprise features without a restart, which a route set decided once at boot could not express.

Booting with no `YORISHIRO_LICENSE_KEY` logs "no licence key configured: enterprise features are disabled" and serves every community route unchanged; booting with a valid key logs "licence key accepted: enterprise features are enabled".

**One binary carries both editions**, so a deployment cannot be identified by which artifact it installed and there is no on-disk separation to assert against. What a deployment serves is decided at runtime.

## Gated routes

**Which routes are gated is narrower than "everything under `ee/`".**

Gated (return 404 without licence key):
- `marketplace`
- `stripe`
- `oauth`
- `inference::gated_routes` (`infer-fill` alone)

Ungated (serve regardless):
- `dashboard`
- `embedding`
- `entity_columns`
- `origin`
- `worker_class`
- `inference`'s `/workspace/llm-key` routes

**`stripe` and `oauth` are gated because billing and SSO login are enterprise-edition features.** The webhook still verifies its Stripe signature; both do nothing until their own configuration is set. The consequences are meant: a deployment that has not bought a licence cannot receive Stripe webhooks, and cannot serve OAuth login either, so anyone who signs in that way needs a password credential for `POST /auth/login` instead.

## Licence tests

Tests live in `tests/requests/licence_gate.rs`, covering verification, expiry-boundary exclusivity (`exp > now`, not `>=`), and config-file key parsing. The suite generates its own throwaway RSA keypair per test via `openssl genrsa`/`openssl rsa -pubout` into a `tempfile::TempDir`, never a checked-in `.pem`: a committed private key reads as a leaked secret to a scanner regardless of what it actually signs, so nothing under `tests/` is a key file.

## Config file

This application resolves `config/{environment}.yaml` and reads the licence from `initializers.licence_key`.
Shipped configurations interpolate `YORISHIRO_LICENSE_KEY` into that field.
Runtime verification consumes the resolved configuration, not the environment.
