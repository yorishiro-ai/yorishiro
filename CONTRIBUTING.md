# Contributing

## Where code goes

The layout follows Laravel's MVC, with Rust's module system standing in for its directories.

| Laravel | Here | Holds |
|---|---|---|
| `app/Models/` | `crates/yorishiro-core/src/models/` | Record shapes, input DTOs, and the queries that read and write them |
| `app/Http/Controllers/` | `crates/yorishiro-server/src/http/controllers/` | What a request means, and what to do about it |
| `routes/` | `crates/yorishiro-server/src/routes.rs` | The URL to controller map |
| `app/Services/` | `crates/yorishiro-core/src/services/` | Decisions that outlive one request: auth, embedding, queueing |
| `resources/views/` | `ee/web/` | The SPA |
| `database/migrations/` | `migrations/` | Schema versioning |
| `database/seeders/` | `crates/yorishiro-core/templates/*.json` | Seed data |

The paid edition mirrors the first four under `ee/crates/yorishiro-hosted/src/`, for the tables and endpoints it adds.

### A model owns a table

Its shape and its queries live in the same module, the way an Eloquent model carries both.
There is no repository layer: `repositories` names a pattern, not a layer, and a directory named after it left `models` holding nothing but structs while every query sat next door.

If you are adding a table, add a module under `models/`.
If you are adding a decision that reaches for a table, put the decision in `services/` and the query in `models/`.

### What is not a model

`migrations/`, `templates/*.json` and `db.rs` are the database's concerns, not a model's, and they stay outside `models/` on purpose.

### Where the queries are

A new table's queries belong in `models/`.
Some `sqlx::query` calls outside it are correct and stay:

- `crates/yorishiro-core/src/db.rs` and `services/db_load_guard.rs` are connection handling, not table access
- `crates/yorishiro-server/src/http/controllers/health.rs`'s `SELECT 1` is a liveness probe with no table to belong to
- `crates/yorishiro-core/src/services/auth/` reads keys as part of deciding a request's identity, which is a decision rather than a record

Others are known debt rather than intent, and are being moved a few at a time:
`crates/yorishiro-server/src/admin/commands.rs`, `http/controllers/setup/mod.rs`, `services/embedding/sync/`,
and in `ee/`, `services/marketplace.rs`, `official_templates.rs`, `tenant_auth.rs`, `oauth/users.rs`, `origin.rs` and `http/controllers/inference.rs`.
If you are already editing one, moving its queries into `models/` is welcome.
If not, leave it: a move with no reason to touch the file is a diff nobody can review against a behaviour change.

## Tests mirror src, one to one

`tests/` reproduces `src/`'s tree exactly.
`crates/yorishiro-core/src/models/schemas/mod.rs` is tested by `crates/yorishiro-core/tests/models/schemas/mod.rs`, and nothing else.

The wiring is an include, not an integration test:

```rust
#[cfg(test)]
#[path = "../../../tests/models/schemas/mod.rs"]
mod tests;
```

Every crate sets `autotests = false`, so a file under `tests/` that no `#[path]` names is compiled by nothing.
It will not fail; it will simply never run.
When you move a source file, move its test file the same way and fix the `#[path]` depth.

## Before you push

```sh
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo test --workspace
pnpm --dir ee/web run check   # only if ee/web changed
```

The test suite needs PostgreSQL with `vector` and `pg_trgm` installed in `template1`, and a role that is **not** a superuser.
A superuser bypasses RLS regardless of `FORCE`, so a green run as one proves nothing about isolation.

## Browser tests

`cargo test` does not run them.
They need a licensed server with a schema and entities in it, plus a chromedriver, and a suite that silently passes when its dependencies are missing is worse than one that is not run at all.
So every test in `ee/crates/yorishiro-hosted/tests/e2e/` is `#[ignore]` and asks for its deployment by name:

```sh
chromedriver --port=9515 &
YORISHIRO_E2E_URL=http://localhost:18081 \
  YORISHIRO_E2E_EMAIL=you@example.com \
  YORISHIRO_E2E_PASSWORD=... \
  cargo test -p yorishiro-hosted --test e2e -- --ignored --test-threads=1
```

**The driver's major version must match the browser's.** A 152 driver refuses to start a session against Chrome 151, and the error names the versions, so read it rather than assuming the driver is missing.

They are not in CI.
Doing it properly needs a licence key as a repository secret and a seeded deployment, and a job that cannot actually run is worse than no job: it reports green having done nothing.

The suite signs in once and shares the session.
The auth limit is 10 requests per minute per IP across `/auth/login`, `/auth/signup` **and** `/setup`, and the SPA checks `/setup` on load, so page loads spend the same budget as sign-ins.
When the limit is hit the suite waits out the window rather than the deployment weakening a limit for the benefit of a test, which is why a run can take 76 seconds instead of 14.

## Prose in this repository

One sentence per line, in Markdown and in comments.
Do not hard-wrap, and do not join two clauses with a dash.

`migrations/` is exempt from every cosmetic rule: sqlx checksums each file including its comments, so editing an applied migration stops the server from starting.
