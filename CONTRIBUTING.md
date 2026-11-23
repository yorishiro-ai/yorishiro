# Contributing

User documentation: [English and Japanese wiki](https://yorishiro-ai.github.io/wiki/).

## Code layout

| Location | Holds |
|---|---|
| `src/models/` | Table-owned persistence and domain operations |
| `src/controllers/` | Request handlers |
| `src/dtos/` | Request and response types |
| `src/services/` | External embedding clients |
| `migration/src/` | Schema migrations |
| `src/tasks/` | Admin and one-off commands |
| `ee/` | Enterprise edition (mirrors `models/`, `controllers/`, `services/`) |
| `src/lib.rs` | Crate-root composition: the only `Hooks` implementation and overlay assembly |
| `src/templates/` | Built-in JSON template assets embedded by `src/data/templates.rs` |
| `.github/packaging/` | Package definitions, hooks, service units and verification scripts |

- Only `src/lib.rs` assembles `ee/`; other base modules have no enterprise branches or references, enforced by `make edition-boundary-check`.
- `src/models/_entities/` is auto-generated. Never edit by hand.
- Table extensions live in `src/models/<table>.rs`; large ones may contain private submodules.
- Business logic goes in `src/models/<table>.rs` next to the generated entity.
- Use SeaORM by default; explain raw SQL where backend primitives, atomic writes, or query shape require it.
- `src/db.rs` handles connections, not tables.

### Database pools

Database connection pools are unrelated to worker pools and the queue.
On PostgreSQL, `AppContext.db` and `DbHandle.identity` share the migration-role pool; `TenantDb` is the separate RLS pool that sets `ROLE yorishiro_app`.
The Loco queue provider owns another independent pool.
SQLite has no `DbHandle`; it uses `AppContext.db` plus the queue provider's separate SQLite pool.

Production defaults to at most 10 connections on SQLite and 100 on PostgreSQL.
Pool settings live in `config/production.yaml` and `config/test.yaml`; CI supplies its overrides in `.github/workflows/ci.yml`.
These limits apply per independent pool, so aggregate database connections are the sum of the pools a process creates.
Async waits release the task to do other work, but one physical connection still executes one SQL operation at a time.

## Adding a table

1. Write a migration under `migration/src/`.
2. Run `make entities` to regenerate `_entities/`.
3. Add a module under `src/models/`.

## Tests

Run the local development stack with `docker compose -f docker-compose.test.yaml up --build`.
The Makefile starts the test database or Valkey from the same Compose file when using the default local URLs.
The separate `docker-compose.yaml` deploys published images and requires a release `TAG` and a production `DATABASE_URL`.
`YORISHIRO_HTTP_ALLOWED_NETWORKS` restricts REST (including webhooks), MCP, Swagger, authentication, and setup to a JSON list of peer CIDRs.
The default `[]` allows all peers without bypassing authentication.
Behind a reverse proxy, enforce client IP policy at the proxy because the app uses the socket peer and ignores forwarding headers.

Use `make test-sqlite` or `make test-postgres` locally; `EDITION=ce` selects the community edition.
The backend comes from `DATABASE_URL` and the edition from the feature set; there is no other test selector.
`RUST_TEST_THREADS` controls test parallelism; CI limits the unlicensed EE PostgreSQL job to two threads.

Request tests boot through `boot_request`: SQLite gets its own database file and PostgreSQL its own throwaway database per test, both torn down with the test.
An app booted by other means must call `close_app_pools` before it returns, or teardown panics even on a passing test.

Licensed tests skip without `YORISHIRO_LICENSE_KEY`, and the E5 model tests skip without `models/multilingual-e5-base/`.
Stripe tests run in the ordinary EE suite, install their own test licence state and require no Stripe account.
`make test-python` runs the Python checks, and `make test-contract` runs the real-binary public contracts described in `tests/blackbox/README.md`.
Run the optional SQLite search benchmark with `cargo bench --bench sqlite_ann -- sqlite:///path/to/database.sqlite3 WORKSPACE_UUID`; it opens the database read-only.

Metaschema validation includes property-based tests for arbitrary schema-shaped definitions and rejection of empty `entity_types`.

The main CI workflow runs when pull requests open, reopen, or receive new commits, and on pushes to `develop`.
It also supports merge queue checks targeting `develop` if the queue is enabled.
Licensed checks run on trusted same-repository pull requests and pushes to `develop`, not fork pull requests or merge queue refs.
If an external run cannot restore the verified E5 model cache, it still runs the tests that do not require the model.

## Before you push

Linux builds use the repository `.cargo/config.toml`, which requires `mold` and `sccache`.
Install both tools before running Cargo locally.

```console
$ make check-all
$ make check-all-ee
```

Tests need PostgreSQL with `vector` and `pg_trgm` in `template1`, and a non-superuser role.

## Git workflow

Branch from `develop`.
For an independent change, open a single pull request against `develop`.
For dependent changes in this repository, prefer a [stack of small pull requests](https://docs.github.com/en/pull-requests/get-started/about-stacked-prs).
GitHub does not support cross-fork stacks, so a regular pull request from a fork is welcome.
Merge commits preferred.
User-facing documentation lives in the [wiki](https://github.com/yorishiro-ai/wiki) and is managed separately from code changes.

## Rules

Rules for code style live at `.agents/rules/`.
