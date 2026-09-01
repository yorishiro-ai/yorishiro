# Tests

**Loco's own plain `tests/` integration-test crate, not a `#[path = "..."]` bridge pattern.**
`src/db.rs`'s production path (`TenantDb::connect`) runs `SET ROLE yorishiro_app` unconditionally in `after_connect`, so a test helper needs no special access to exercise RLS.
It just calls `TenantDb::connect(url, max_connections)` against a real test database the same way `Hooks::after_context` does.
`config/test_postgres.yaml`'s `database.uri` already reads `DATABASE_URL` with `auto_migrate: true`, so `cargo test` against a scratch Postgres runs converge and gets a role-and-RLS-correct connection with no bridge, no `#[path]`, no `autotests = false`.
`TenantDb::new(pool)` still exists (it bypasses `after_connect`, so it skips `SET ROLE`) but nothing calls it and nothing should: `connect` is the only path a test needs.
**Test layout**: the integration tests are one binary rooted at `tests/mod.rs`, which declares the submodules (`mod requests;`, `mod models;`, `mod tasks;`).
Shared helpers live in the submodule that owns them and are reached by path: `close_app_pools` sits in `tests/requests/mod.rs` and `tests/models/*` calls it as `crate::requests::close_app_pools`.
There is no `tests/lib.rs` and no `tests/test_helpers.rs`, in either crate; a previous version of this rule described both, and neither has ever existed here.

`close_app_pools` has one definition, in `tests/requests/mod.rs`: one crate means one integration-test binary, so every suite reaches it by path.

**A boot failure inside `request_with_create_db` surfaces as a `DROP DATABASE` panic during `loco_rs`'s own cleanup, not as the actual boot error.**
`after_context` opens the identity pool eagerly, before `db::converge` runs; if `converge` then fails, that pool still holds a session on the throwaway test database, and `loco_rs::testing::db::PostgresTest::cleanup_db`'s `DROP DATABASE` fails with "being accessed by other users", panics, and the real `H::boot` error is swallowed (`loco_rs-1.1.0/src/testing/request.rs:218`, inside the `Err(err)` arm).
**Read that as "why did boot fail", not "find the leaked pool"**: `close_app_pools` in `tests/requests/mod.rs` closes every pool this app opens, and is required before assuming a `DROP DATABASE` panic is the actual failure rather than a symptom of one.

**Loco's request-test harness creates each throwaway database with `CREATE DATABASE`, which copies `template1`.**
A fresh Postgres volume needs `vector`/`pg_trgm` installed into `template1` itself (not just the deployment's main database), or every throwaway test database inherits a `template1` with neither, and `converge` fails on `content_entities.embedding` inside every request test for exactly the boot-failure-looks-like-cleanup-panic reason above.
Check this at its own layer (`psql -d template1 -c '\dx'`), not assumed: a fresh volume missing the fix shows `template1` holding only `plpgsql`.

**A request test that boots through `request_with_create_db` must call `close_app_pools` before its closure returns, or teardown panics even on a passing test.**
`after_context` opens two pools Loco's harness doesn't know about (identity, eager; tenant, lazy), and `config/test_postgres.yaml`'s `min_connections: 1` keeps one connection open on `ctx.db` itself; none of the three close on their own when the closure returns.
`close_app_pools` in `tests/requests/mod.rs` is the pattern every request test copies.
The Postgres queue provider (`config/test_postgres.yaml`'s `queue:` block) is a fourth pool this same way, and it has no public close path at all (`shutdown()` only cancels its polling loop), so `queue:` is **omitted entirely from `config/test_postgres.yaml`**: nothing in this codebase enqueues a job yet (`connect_workers` is a no-op), so no test needs one, and there is no fix on the closing side for a pool with no closing method.

**A gate is not a gate until a deliberate violation makes it fire.**
`redeem_invite`'s race-safety claim (two concurrent redemptions of the same token can't both succeed) needs two racing redemption calls behind a barrier to actually test the race; a sequential replay-rejection test only proves the upfront `SELECT` filters correctly, not that the `UPDATE ... WHERE used_at IS NULL` guard is race-safe.
The same shape applies to any advisory-lock gate (a quota lock, a version-serialization lock) and to a foreign-key-violation branch that depends on a delete happening between a check and an insert: none of these are proven by a happy-path unit test, only by a widened-race test with multiple concurrent callers behind a barrier.

**A fresh database needs its role created before any migration's `helpers::grant` runs.**
Every migration file's `grant` helper assumes `yorishiro_app` already exists; verify this by migrating a fresh volume as a non-superuser role and confirming `SET ROLE yorishiro_app` succeeds without escalation, since a superuser migrating role can `SET ROLE` regardless of grant membership and would mask a missing grant.

**Adding or removing an MCP tool breaks the tool-count assertion and whatever test builds dummy arguments per tool name.**
Both need updating in the same change as the tool list.
