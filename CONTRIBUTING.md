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
On PostgreSQL, `AppContext.db` is Loco's SeaORM pool using the migration role, `TenantDb` is the separate RLS pool that sets `ROLE yorishiro_app`, and `DbHandle.identity` is a second migration-role raw sqlx pool.
The Loco queue provider owns another independent pool.
SQLite has no `DbHandle`; it uses `AppContext.db` plus the queue provider's separate SQLite pool.

Production defaults `database.max_connections` to `100` on either backend unless configuration overrides it.
The minimum default is `1`.
CI overrides PostgreSQL `DB_MAX_CONNECTIONS` to `100`, and the SQLite test configuration sets its application pool maximum to `10`.
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
Set `YORISHIRO_HTTP_ALLOWED_NETWORKS` to a JSON array of CIDRs, such as `["127.0.0.0/8", "::1/128"]`, to restrict REST, MCP, Swagger UI, signup, and setup to the same peer networks.
An empty list preserves unrestricted network access; authentication still applies where required.
Behind a reverse proxy, enforce client IP policy at the proxy and allow only the proxy network at the application.
The application does not trust forwarded IP headers.
If external webhooks are enabled, their requests must also come through an allowed peer.

Run the full suite with Rust's default parallel test execution.

Use `make test-sqlite` or `make test-postgres` locally; `EDITION=ce` selects the community edition.
The backend comes from `DATABASE_URL` and the edition from the feature set; there is no other test selector.

Request tests boot through `boot_request`: SQLite gets its own database file and PostgreSQL its own throwaway database per test, both torn down with the test.
An app booted by other means must call `close_app_pools` before it returns, or teardown panics even on a passing test.

Licensed tests skip without `YORISHIRO_LICENSE_KEY`, and the E5 model tests skip without `models/multilingual-e5-base/`.
Run the opt-in Stripe tests with `cargo test --locked --features stripe-tests --test mod -- ::stripe::`.
CI runs these separately from general coverage because they exercise inbound signed webhooks.
Use [stripe-mock](https://github.com/stripe/stripe-mock) for outbound Stripe API contract tests when an outbound client is added; it does not send signed webhooks.
The Stripe request tests install a test licence state and do not require a Stripe account or a signed EE licence.
`UPDATE_FIXTURES=1 cargo test --features enterprise enterprise_inventory_contract` rewrites `tests/fixtures/mcp-inventory.md`; review the diff before committing.
`UPDATE_FIXTURES=1 cargo test --features enterprise openapi_document_is_mounted_with_enterprise_routes` updates `tests/fixtures/openapi-operations.txt` with the paths, methods, parameters, and response codes from the served OpenAPI document.
Run the optional SQLite search benchmark with `cargo bench --bench sqlite_ann -- sqlite:///path/to/database.sqlite3 WORKSPACE_UUID`; it opens the database read-only.

Metaschema validation includes property-based tests for arbitrary schema-shaped definitions and rejection of empty `entity_types`.

The main CI workflow runs when pull requests open, reopen, or receive new commits, and on pushes to `develop`.
It also supports merge queue checks targeting `develop` if the queue is enabled.
Licensed checks are not run against fork pull requests or merge queue refs; the full licensed suite runs after merge on `develop`.
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
