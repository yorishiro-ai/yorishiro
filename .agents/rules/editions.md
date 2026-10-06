# Editions and the `ee/` boundary

## Licences

One repository, two licences. Everything outside `ee/` is BUSL-1.1; `ee/` is the enterprise edition under `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.

## Boundary

**Nothing in `src/` may refer to the enterprise overlay.** That means no `cfg(feature = "enterprise")`, no `cfg(not(feature = "enterprise"))`, no `crate::ee`, no path into `ee/`, and no overlay names, queues, branches, or registrations, comments included. The dependency runs from `ee/` into `src/`, never back.

The composition root is `edition/`, outside `src/` on purpose. It is the only place that knows which editions exist, and it is assembly only. One crate makes both directions compile, so the boundary is a rule rather than a compiler error: `scripts/check_edition_boundary.py` is the mechanical guard, run by `make edition-boundary-check`, `make check-all`, `make check-all-ee`, and CI. It has tests that include deliberate violations.

## Classification

Which side a feature belongs on is decided by what the feature *is*, never by what it needs.

**The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.

"The user brings their own key" does not move a feature out of `ee/`, because it changes who pays rather than what the server does. Any sentence of the form "it does not depend on X" is the wrong test.

Unclear cases are a question for the user, asked as the classification itself rather than buried in the options of an implementation question.

## Composition

**`ee/` is a module of the application crate, not a package of its own.** `edition/mod.rs` declares it with `#[path = "../ee/mod.rs"]` behind the `enterprise` feature, and `src/lib.rs` declares `edition/` itself with `#[path = "../edition/mod.rs"]` and no feature gate. The files stay at the repository root where `ee/LICENSE` scopes them ("everything under the `ee/` directory") while compiling into the same crate as everything in `src/`. The overlay is therefore `crate::edition::ee`. The root `Cargo.toml`'s `[workspace] members` lists `"."` only; there is one binary, `yorishiro`.

One crate is also what lets loco's own logging reach this application at all: its default filter is a fixed module whitelist plus exactly one `Hooks::app_name()` entry (`logger.rs:192-210`), so a second application crate would be silent unless every `config/*.yaml` named it in `override_filter`.

**There is one `Hooks` impl, `edition::App` in `edition/app.rs`, re-exported as `yorishiro::App`.** Each hook calls the matching function in `src/app.rs` and then the active edition's function: `after_context` installs the licence state, the tenant-scoped authenticator (PostgreSQL only) and the resolver and policy seams after building base's own pools; `routes()` adds the enterprise edition's route groups after the community ones through `RouteMounts`; `register_tasks` registers `ee/`'s tasks alongside base's; the MCP tools and their licence policy are added through `McpToolSet`.

The active edition is `ee/app.rs` when the `enterprise` feature is on and `edition/community.rs` otherwise. They expose identical functions, so the build of whichever edition fell behind fails on a signature mismatch. There is no trait between them: one overlay and no runtime choice is not a reason for a trait. A trait is for behaviour that varies at runtime, such as `QueuePolicy`, `Authenticator`, and `WorkerClassResolver`, which `ee/` replaces by inserting its own `Arc<dyn Trait>` into `shared_store`.

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

This application resolves `config/{environment}.yaml` and defines no `license_key:` field there.
The only live configuration is the `YORISHIRO_LICENSE_KEY` environment variable.
