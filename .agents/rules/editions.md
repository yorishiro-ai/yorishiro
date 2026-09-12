# Editions and the `ee/` boundary

## Licences

One repository, two licences. Everything outside `ee/` is BUSL-1.1; `ee/` is the enterprise edition under `ee/LICENSE`, which adds a Competing Use restriction and requires a licence key for production use.

## Boundary

**`src/app.rs` is the only file in `src/` that may reference `crate::ee`.** Everywhere else the dependency runs from `ee/` into `src/`, never back.

`app.rs` is the composition root: `after_context` installs `ee/`'s seams and `routes()` mounts its routes. This is wiring, not a feature depending on a feature. One crate makes both directions compile, so this is a rule rather than a compiler error: `use crate::ee::...` in any other `src/` file is the import direction that is always wrong.

## Classification

Which side a feature belongs on is decided by what the feature *is*, never by what it needs.

**The server calling an LLM**, billing, external SaaS and rich UI are `ee/` by character.

"The user brings their own key" does not move a feature out of `ee/`, because it changes who pays rather than what the server does. Any sentence of the form "it does not depend on X" is the wrong test.

Unclear cases are a question for the user, asked as the classification itself rather than buried in the options of an implementation question.

## Composition

**`ee/` is a module of the application crate, not a package of its own.** `src/lib.rs` declares it with `#[path = "../ee/mod.rs"]`, so the files stay at the repository root where `ee/LICENSE` scopes them ("everything under the `ee/` directory") while compiling into the same crate as everything in `src/`. The root `Cargo.toml`'s `[workspace] members` lists `"."` only; there is one binary, `yorishiro`.

One crate is also what lets loco's own logging reach this application at all: its default filter is a fixed module whitelist plus exactly one `Hooks::app_name()` entry (`logger.rs:192-210`), so a second application crate would be silent unless every `config/*.yaml` named it in `override_filter`.

**There is one `Hooks` impl, `app::App`.** `ee/`-only wiring is layered inside its methods rather than composed from a second impl: `after_context` installs the licence state, the tenant-scoped authenticator (PostgreSQL only) and the two resolver seams after building base's own pools; `routes()` adds the enterprise edition's route groups after the community ones; `register_tasks` registers `ee/`'s two tasks alongside base's.

`YorishiroError` is re-exported at the crate root (`pub use error::YorishiroError;`), so `ee/` code reaches it as `crate::YorishiroError`.

## Licence gate

**The licence gate is a per-request layer, not a compilation boundary.** `ee/services/licence.rs` verifies a signed licence key against `ee/keys/licence-public.pem`. `app::licence_gate` reads that state on every request and answers 404 on the routes it is attached to, applied through `Routes::layer` so it reaches exactly those routes and cannot leak onto the community ones.

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
