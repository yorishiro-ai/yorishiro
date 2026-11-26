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
- Table-owned persistence and business logic live in `src/models/<table>.rs`; large models may contain private submodules.
- Use SeaORM by default; explain raw SQL where backend primitives, atomic writes, or query shape require it.
- `src/db.rs` handles connections, not tables.

### Database pools

Database connection pools are unrelated to worker pools and the queue.
On PostgreSQL, `AppContext.db` and `DbHandle.identity` share the migration-role pool; `TenantDb` is the separate RLS pool that sets `ROLE yorishiro_app`.
The Loco queue provider owns another independent pool.
SQLite has no `DbHandle`; it uses `AppContext.db` plus the queue provider's separate SQLite pool.

Pool limits and timeouts live in `config/production.yaml` and `config/test.yaml`; CI supplies its overrides in `.github/workflows/ci.yml`.
These limits apply per independent pool, so aggregate database connections are the sum of the pools a process creates.

## Adding a table

1. Write a migration under `migration/src/`.
2. Run `make entities` to regenerate `_entities/`; it needs Docker and the `sea-orm-cli` version pinned in `Cargo.lock`.
3. Add a module under `src/models/`.

## Tests

Run the local development stack with `docker compose -f docker-compose.dev.yaml up --build`.
The Makefile starts only test dependencies from `docker-compose.test.yaml` when using the default local URLs.
The separate `docker-compose.yaml` deploys published images and requires a release `TAG` and a production `DATABASE_URL`.

Use `make test-sqlite` or `make test-postgres` locally; `EDITION=ce` selects the community edition.
The backend comes from `DATABASE_URL` and the edition from the feature set; there is no other test selector.
`RUST_TEST_THREADS` controls test parallelism; CI limits the unlicensed EE PostgreSQL job to two threads.

Request tests boot through `boot_request`: SQLite gets its own database file and PostgreSQL its own throwaway database per test, both torn down with the test.
An app booted by other means must call `close_app_pools` before it returns, or teardown panics even on a passing test.

Licensed tests install a test licence when `YORISHIRO_LICENSE_KEY` is unset, and the E5 model tests skip without `models/multilingual-e5-base/`.
Stripe tests run in the ordinary EE suite, install their own test licence state and require no Stripe account.
Run the optional SQLite search benchmark with `cargo bench --bench sqlite_ann -- sqlite:///path/to/database.sqlite3 WORKSPACE_UUID`; it opens the database read-only.
Run the PostgreSQL benchmark with `cargo bench --bench postgres_search -- DATABASE_URL TENANT_UUID WORKSPACE_UUID QUERY_VECTOR_FILE`, using a JSON file of vector coordinates and an existing populated workspace.
The connection role must be allowed to assume `yorishiro_app`; the benchmark applies that restricted role and transaction-local tenant/workspace scope inside a read-only transaction.

Licensed checks run on trusted same-repository pull requests and pushes to `develop`, not fork pull requests or merge queue refs.
If an external run cannot restore the verified E5 model cache, it still runs the tests that do not require the model.

## Before you push

Linux builds use the repository `.cargo/config.toml`, which requires `mold` and `sccache`.
Install both tools, and `uv` for the Python checks, before running Cargo locally.

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
