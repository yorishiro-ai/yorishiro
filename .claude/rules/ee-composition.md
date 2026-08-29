# `ee/` composition

**`ee/` is a module of the application crate, not a package of its own.**
`src/lib.rs` declares it with `#[path = "../ee/mod.rs"]`, so the files stay at the repository root where `ee/LICENSE` scopes them ("everything under the `ee/` directory") while compiling into the same crate as everything in `src/`.
The root `Cargo.toml`'s `[workspace] members` lists `"."` and `"migration"`; there is one binary, `yorishiro`.

One crate is also what lets loco's own logging reach this application at all: its default filter is a fixed module whitelist plus exactly one `Hooks::app_name()` entry (`logger.rs:192-210`), so a second application crate would be silent unless every `config/*.yaml` named it in `override_filter`.

**There is one `Hooks` impl, `app::App`.**
`ee/`-only wiring is layered inside its methods rather than composed from a second impl: `after_context` installs the licence state, the tenant-scoped authenticator (PostgreSQL only) and the two resolver seams after building base's own pools; `routes()` adds the paid edition's route groups after the community ones; `register_tasks` registers `ee/`'s two tasks alongside base's.
`YorishiroError` is re-exported at the crate root (`pub use error::YorishiroError;`), so `ee/` code reaches it as `crate::YorishiroError`.

**The licence gate is a per-request layer, not a compilation boundary.**
`ee/services/licence.rs` verifies a signed licence key against `ee/keys/licence-public.pem`.
`app::licence_gate` reads that state on every request and answers 404 on the routes it is attached to, applied through `Routes::layer` so it reaches exactly those routes and cannot leak onto the community ones.
Per request rather than at boot is deliberate: `LicenceState::is_active` compares `exp` against the current clock, so a key that lapses while the process runs stops unlocking paid features without a restart, which a route set decided once at boot could not express.

Booting with no `YORISHIRO_LICENSE_KEY` logs "no licence key configured: paid features are disabled" and serves every community route unchanged; booting with a valid key logs "licence key accepted: paid features are enabled".

**One binary carries both editions**, so a deployment cannot be identified by which artifact it installed and there is no on-disk separation to assert against.
What a deployment serves is decided at runtime, and `tests/requests/licence_gate.rs` is where that is checked: gated routes answer 404 unlicensed and are served licensed, ungated ones stay reachable in both boots.

**Which routes are gated is narrower than "everything under `ee/`".**
`marketplace`, `stripe`, `oauth` and `inference::gated_routes` (`infer-fill` alone) carry the gate.
The rest (`dashboard`, `embedding`, `entity_columns`, `origin`, `worker_class`, and `inference`'s `/workspace/llm-key` routes) serve without a licence.

**`stripe` and `oauth` are gated because billing and SSO login are paid-edition features.**
This decides the question the way `editions.md` says to decide it, by what each feature is rather than by what protects it.
An earlier version of this file argued the other way for both: that the Stripe webhook must stay open because buying a licence goes through it, and that `oauth` needed no gate because it is opt-in by setting an issuer URL.
Both were considered and rejected, as claims about what a route depends on rather than about what it is, which is the shape of argument `editions.md` rules out.
Neither route lost a protection it had: the webhook still verifies its Stripe signature, and both still do nothing until their own configuration is set.

The consequences are meant, not incidental.
A deployment that has not bought a licence cannot receive Stripe webhooks, and cannot serve OAuth login either, so anyone who signs in that way needs a password credential for `POST /auth/login` instead.

**Licence tests** live in `tests/licence.rs`, covering verification, expiry-boundary exclusivity (`exp > now`, not `>=`), and config-file key parsing.
The suite generates its own throwaway RSA keypair per test via `openssl genrsa`/`openssl rsa -pubout` into a `tempfile::TempDir`, never a checked-in `.pem`: a committed private key reads as a leaked secret to a scanner regardless of what it actually signs, so nothing under `tests/` is a key file.

**The config-file fallback (`licence_key_from_config` in `licence.rs`) is dead code.**
It reads `config.yml`/`YORISHIRO_CONFIG_PATH`; this application resolves `config/{environment}.yaml` and defines no `license_key:` field there, so the function always returns `None` and only the `YORISHIRO_LICENSE_KEY` environment variable is live.
