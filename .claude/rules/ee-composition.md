# `ee/` composition on the Loco rebuild

**Decided (2026-08-22): base stays flat, `ee/crates/yorishiro-hosted` is the one added workspace member.**
Master's `crates/yorishiro-core`/`crates/yorishiro-server` split does not exist on this branch, and `ee/crates/yorishiro-hosted` there depended on both, so neither `git merge master` nor copying `ee/` wholesale produces something that compiles against the rebuilt layout.
The root `Cargo.toml`'s `[workspace] members` lists `"."` (the root package is `yorishiro-core` itself) and `"ee/crates/yorishiro-hosted"`.

**The seam is Loco's own `Hooks` trait, not the five sqlx-era contracts that died with the sqlx layer.**
`ee/crates/yorishiro-hosted/src/lib.rs` defines `HostedApp`, a second `Hooks` impl distinct from `yorishiro_core::app::App`.
Every method delegates to the matching associated fn on `App` first (`App::routes(ctx)`, `App::after_context(ctx).await`, and so on), because `Hooks`'s methods take no `self`, so they compose by direct call rather than trait inheritance.
`ee/`-only behaviour is layered around that call, not duplicated inside it: `HostedApp::after_context` calls `App::after_context` for the RLS pool and authenticator seam, then resolves and stores the licence state on top.
The bin `ee/crates/yorishiro-hosted/src/bin/yorishiro_server.rs` calls `cli::main::<HostedApp, Migrator>()`, mirroring `yorishiro-core`'s own `bin/main.rs` with `HostedApp` in place of `App`.
`yorishiro_core::error::YorishiroError` is re-exported at the crate root (`pub use error::YorishiroError;` in `src/lib.rs`), giving `ee/` code (and anything else outside the crate) a short import path to it.

**The licence gate**: `ee/crates/yorishiro-hosted/src/services/licence.rs` verifies a signed licence key (`ee/LICENSE`, `keys/licence-public.pem`).
Booting with no `YORISHIRO_LICENSE_KEY` logs "no licence key configured: paid features are disabled" and serves every base route unchanged; booting with a valid key logs "licence key accepted: paid features are enabled".
Tests: `ee/crates/yorishiro-hosted/tests/licence.rs`, covering verification, expiry-boundary exclusivity (`exp > now`, not `>=`), and config-file key parsing.
The suite generates its own throwaway RSA keypair per test via `openssl genrsa`/`openssl rsa -pubout` into a `tempfile::TempDir`, not a checked-in `.pem`: a committed private key reads as a leaked secret to a scanner regardless of what it actually signs, so nothing under `tests/` is a key file.
The edition boundary is checked at its own layer, not assumed: `grep -ac YORISHIRO_LICENSE_KEY target/debug/yorishiro_core-cli` must answer 0, and the same grep against `target/debug/yorishiro-server` must answer 1, confirming the licence string exists only in the binary that should carry it.

**The config-file fallback (`licence_key_from_config` in `licence.rs`) is currently dead.**
It reads `config.yml`/`YORISHIRO_CONFIG_PATH`, master's pre-rebuild server config convention; the Loco rebuild resolves `config/{environment}.yaml` instead and defines no `license_key:` field there, so this function always returns `None` until a Loco config field is wired up for it, and only the `YORISHIRO_LICENSE_KEY` environment variable is live.
