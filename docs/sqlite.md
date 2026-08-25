# SQLite

**English** | [日本語](ja/sqlite.md)

Yorishiro's migrations (`migration/`) produce a valid schema on SQLite as well as PostgreSQL.
This document describes what that currently covers and what it does not.

## Status: schema, the single-tenant guard, and most authenticated routes

`Migrator::up` against a SQLite URL produces a correct and complete schema, `tenancy::create_tenant` enforces a hardcoded single-tenant cap independent of `YORISHIRO_MAX_TENANTS`, and the application itself boots against a SQLite file and serves `POST /setup`, `GET /api/whoami`, and every `Authorized<R>`/`AuditAuthorized` route that doesn't reach `content_entities` (see "What's still blocked" below for the exact boundary).
Setup creates the deployment's one tenant/workspace/user/API key; `/whoami` authenticates that key and returns the identity it resolves to; `POST /auth/signup` (the other path that can create a tenant) is refused with `409` and the SQLite-specific remedy message once the cap is reached, the same as a second `/setup` call.

`AuthContext`, `Authorized<R>`, and `AuditAuthorized` all have a SQLite branch (`src/controllers/extractors.rs`), authenticating and (for the latter two) opening a plain transaction directly against `ctx.db` rather than going through `DbHandle`/`TenantDb::begin_for_workspace`, since that Postgres-only two-pool RLS machinery has nothing to scope on a single-tenant backend with no RLS.
`Verified<R>` deliberately has no SQLite branch: its one caller (`search_entities`) calls `db_handle()` directly regardless, and the route is unreachable on SQLite anyway since it depends on `content_entities.embedding`, which doesn't exist on that backend (see "What's still blocked").

`config/sqlite.yaml` is a manual-verification environment (`LOCO_ENV=sqlite`), not wired into any test suite; `tests/` stays PostgreSQL-only.
It has no `queue:` block (`ForegroundBlocking` workers), and expects `YORISHIRO_MAX_TENANTS` to be set for the setup wizard to answer as enabled, the same requirement as any other environment.
The SQLite cap itself ignores the variable's value once the wizard runs, but `wizard_enabled()` still checks that it's set at all before allowing `/setup` to run.

## `database.max_connections` must be at least 2 on SQLite

`Authorized<R>`/`AuditAuthorized` hold one connection open on a transaction for the whole request while separately touching `identity_api_keys.last_used_at` on a second, independent connection from the same pool: kept separate for the same reason PostgreSQL's `authorize`/`touch_last_used_on` do: a read-only handler drops its transaction without committing, and updating `last_used_at` there would silently roll back with it.
At `max_connections: 1` that second acquire has no free connection to get and can only wait out `connect_timeout` before failing.

Booting with `max_connections` below 2 on SQLite is refused outright (`db::require_min_sqlite_connections`, called from `Hooks::after_context`), rather than left to fail unpredictably under load.
Measured before that guard existed, at `max_connections: 1` with `connect_timeout: 500`: a read-only route (`GET /api/relations`) still returned `200`, since the failed `last_used_at` update is best-effort and only logs a warning; a route that itself needs a second connection for a real write (`PUT /api/system/maintenance`, which writes through `ctx.db` independently of the held transaction) failed with `500` after roughly 500ms, logged as `Failed to acquire connection from pool: Connection pool timed out`.
`config/sqlite.yaml` ships with `max_connections: 10`, well above the minimum.

## What SQLite is for

SQLite is scoped to a single tenant.
It has no database-enforced isolation between tenants the way PostgreSQL's row-level security does, so it is meant for trying Yorishiro out or for a single person's own use, not for hosting multiple tenants.
The application-level filtering that would be needed to fake multi-tenant isolation on this engine is deliberately not implemented: a single missed filter in one query would be a silent isolation break, which is exactly what row-level security exists to make structurally impossible on PostgreSQL.

## The single-tenant guard

`tenancy::create_tenant` (`src/models/tenancy.rs`) enforces `YORISHIRO_MAX_TENANTS` against `count_tenants` on PostgreSQL, taking `db::lock_for_update` first to close the count-then-insert TOCTOU gap.
On SQLite, the cap is not read from `YORISHIRO_MAX_TENANTS` at all: it is hardcoded to 1, and the environment variable has no effect on this backend, by design.
Raising `YORISHIRO_MAX_TENANTS` is a way to loosen a configurable policy; on SQLite the limit exists because the isolation mechanism itself (RLS) is absent, not because of policy, so it cannot be configured away.

`db::lock_for_update` (`src/db.rs`) is a no-op on SQLite rather than a substitute lock: SQLite has no equivalent to `pg_advisory_xact_lock`.
This is still race-safe, not merely convenient: SQLite allows only one write transaction at a time, and a transaction that read a stale count and then tries to commit after a different transaction has since written and committed gets `SQLITE_BUSY`, failing the whole transaction rather than committing a second tenant.
The TOCTOU the lock closes on PostgreSQL therefore surfaces as a retryable error on SQLite instead of a silently-accepted inconsistent write.
See the doc comment on `lock_for_update` for the full reasoning, including why this also covers the codebase's other lock call sites that write more than one row per transaction.

Every `uuidv7()`-defaulted `id` column (`identity_tenants`, `identity_workspaces`, `identity_users`, `identity_tenant_memberships`, `identity_api_keys`) generates its own id on SQLite through `ActiveModelBehavior::before_save`, which calls `db::sqlite_generated_id(conn, self.id)`: a no-op when `id` is already `Set` or the backend is PostgreSQL, `Uuid::now_v7()` otherwise.
This covers every plain `ActiveModel::insert()`/`.save()` call, but not `Entity::insert(active).on_conflict(...).exec(conn)`: that builder path does not call `before_save` (confirmed against `sea-orm` 2.0.2's source), so `tenancy::add_member` (the one call site using it) sets `id` explicitly instead of relying on the hook.
A future `on_conflict` insert needs the same explicit treatment; `before_save` alone does not cover it.

## What differs from the PostgreSQL schema, and why

PostgreSQL-only constructs have no SQLite equivalent and are simply absent from the SQLite schema, not replaced with an approximation:

- **Roles, GRANT, row-level security.** A single-tenant, single-file database has no second tenant to isolate from, so there is nothing for a role or a policy to protect.
- **The `authenticate_api_key` SECURITY DEFINER function.** It exists on PostgreSQL only to read rows RLS would otherwise hide from an unauthenticated caller. With no RLS on SQLite, there is nothing to bypass, so the application queries `identity_api_keys`/`identity_workspaces` directly there instead.
- **`uuidv7()` as a column default.** SQLite has no such function, so the `id` column carries no default on that backend; every insert must supply its own id instead. See "The single-tenant guard" above for how the five `uuidv7_pk`-keyed entities handle this via `before_save`.

Some constructs SQLite can express, but not in the same syntax, so the same guarantee is written twice, once per backend:

- **Table-level CHECK constraints.** PostgreSQL migrations add these with `ALTER TABLE ... ADD CONSTRAINT` after the table exists. SQLite's `ALTER TABLE` supports only rename, add-column, and drop-column, so on that backend the same CHECK is declared inline in the `CREATE TABLE` statement instead.
- **The trigger that detaches a schema from a deleted template.** PostgreSQL expresses it as a `plpgsql` function plus a `CREATE TRIGGER` that calls it; SQLite has no separate function/trigger split, so the same `UPDATE` runs directly inside a `CREATE TRIGGER ... BEGIN ... END` body.
- **`identity_templates.tags`.** A PostgreSQL `TEXT[]` array column has no SQLite equivalent; the SQLite column holds the same tag list JSON-encoded as a `TEXT` column instead. Application code reading or writing this column needs to know which representation it is talking to.
- **The `content_entity_column_preferences.columns` array-shape CHECK.** PostgreSQL spells it `jsonb_typeof(columns) = 'array'`; SQLite's JSON1 extension spells the same check `json_type(columns) = 'array'`.

## What is not ported yet: embedding and full-text search

`content_entities.embedding` (the pgvector column) and its associated indexes (HNSW similarity, the GIN JSONB index, the trigram index) do not exist on SQLite.
Vector similarity search and full-text search do not work on this backend yet.
Porting them is expected to land as `sqlite-vec`'s `vec0` virtual table (for vector search) and SQLite's `FTS5` extension (for full-text search), replacing pgvector and `pg_trgm` respectively, but neither has been implemented.

## What's still blocked: anything reaching `content_entities`

`src/models/_entities/content_entities.rs` (generated by `cargo loco db entities`, never hand-edited) declares `embedding: Option<PgVector>` on the `Model` struct unconditionally.
That column exists on PostgreSQL only (see above); the SQLite table has no `embedding` column at all.
The SeaORM entity API builds its queries from every field on `Model`, so any `content_entities::Entity` query (read or write) asks SQLite for a column that isn't there and fails outright.
This is a hard failure, not a silently empty or partial result: measured against `GET /api/entities`, the response is `500 {"error":{"code":"internal","message":"internal server error"}}`, and the server log names the actual cause, `no such column: content_entities.embedding`.
So a caller sees a clear failure rather than "no entities".
The response body itself is a generic `500` rather than a message naming SQLite, so a caller cannot yet distinguish "this backend doesn't support this route" from any other internal error without reading this document or the server's own log.

Blocked as a direct consequence:

- **Entity CRUD** (`POST`/`GET`/`PUT`/`DELETE /api/entities`, `POST /api/migration-jobs/{id}/undo`): reads and writes `content_entities` directly.
- **Export/import** (`GET /api/export.jsonl`, `POST /api/import.jsonl`): same.
- **`GET /api/workspaces/{id}`'s `entity_count` field**: calls `content_entities::count`.
- **`POST /api/relations` (creating a relation)**: `content_relations::create` calls `content_entities::get` on both the source and target id to confirm they exist before linking them.
- **Neighbor traversal** (`content_relations::neighbors_batch`, the batched "entities related to X" lookup): its query `JOIN`s against `content_entities` directly.
- **`GET /api/search`** (`Verified<ReadScope>`): for the independent reason above (`embedding` itself), not this one specifically, though the same table is why.

Not blocked, since they never touch `content_entities`: `GET`/`DELETE /api/relations/{id}`, `PUT /api/relations/{id}/status`, `GET /api/relations` (listing), schemas (`GET`/`POST /api/schemas`, `GET /api/schemas/active/{name}`, `GET /api/schemas/{schema_id}`, templates), `GET /api/audit-log`, and `GET`/`PUT /api/system/maintenance`.
`POST /api/schemas` with a `template_id` body is a partial exception worth knowing about even though it doesn't touch `content_entities`: it calls `identity_templates::resolve_template_definition` on `ctx.db`, a second connection acquired while the request's own transaction is still open (safe under the `max_connections` minimum above, same as `set_maintenance`, not a separate constraint).

Rewriting `content_entities.rs`'s query functions to `select_only().columns([...])` and exclude `embedding` was considered and rejected for now: it would touch ten PostgreSQL request-hot-path functions to support a tier positioned for trial and personal use, and re-introduce a hand-written column list of exactly the kind this codebase's SeaORM migration has been removing elsewhere: a column added to `content_entities` later and missed in one of the ten call sites would fail by silently omitting a field, not by refusing to compile.

## Where the branching logic lives

`migration/src/helpers.rs` holds every backend-conditional migration helper (`enable_rls_with_policy`, `grant`, `pg_only`, `sqlite_only`, `create_table_with_checks`, `uuidv7_pk`), each checking `manager.get_database_backend()`.
A migration file calls these helpers rather than branching on the backend itself.
The resulting PostgreSQL schema (every table, column, constraint name, index, policy and grant) is unchanged from before SQLite support existed; the SQL text emitting some of those constraints was refactored to route through `create_table_with_checks`/`pg_only`; and `identity_maintenance`'s three CHECKs, previously one `execute_unprepared` call holding three semicolon-separated `ALTER TABLE` statements, are now three separate calls with the same effect.

At the application level, `Hooks::after_context` (`src/app.rs`) checks `ctx.db.get_database_backend() != DatabaseBackend::Sqlite` before building `DbHandle`/the default `Authenticator`, and `AuthContext`/`Authorized<R>`/`AuditAuthorized`'s `FromRequestParts` impls (`src/controllers/extractors.rs`) check the same condition to pick between the `..._sqlite` functions in `services::auth::authorize`/`services::auth::authenticate` and the PostgreSQL `Authenticator`/`DbHandle` seam.
`db::sqlite_generated_id` (used from `before_save`, see "The single-tenant guard" above) checks `conn.get_database_backend()` the same way.
`db::require_min_sqlite_connections` (see "`database.max_connections` must be at least 2 on SQLite" above) is the one exception that runs unconditionally on the config value rather than the live connection, since it must reject boot before any connection exists to check.
No other branch anywhere reads a config flag or an environment variable to decide this; every other one is read off the live connection.

## Caveats about the current SQLite path

SQLite does not enforce foreign keys by default; a connection has to run `PRAGMA foreign_keys = ON` itself.
Every `FOREIGN KEY` declaration in the migrated schema is present but inert until whatever connects to the database sets that pragma.

`CURRENT_TIMESTAMP` on SQLite renders as `YYYY-MM-DD HH:MM:SS` (no offset) into a column sea_query names `timestamp_with_timezone_text`.
The column is a plain SQLite `TEXT` column under that name; there is no actual timezone-aware storage on this backend, unlike PostgreSQL's `timestamptz`.
A value written through this column default and one written by the application (`chrono::Utc::now()`, e.g. `touch_last_used_sqlite`'s `last_used_at` update) end up in different textual formats in the same column: `2026-08-24 14:27:08` versus `2026-08-24T15:37:02.437013178+00:00`.
Those happen to compare correctly as timestamps once parsed, but not as strings: nothing in this codebase currently orders by these columns with a raw string comparison, but a future query that does would need to parse first.

`sqlx::postgres::PgPoolOptions::connect` on a `sqlite://` URL does not return an error: it hangs indefinitely (confirmed by direct probe).
This is why `after_context`'s PostgreSQL pool construction is skipped entirely on SQLite rather than attempted and expected to fail fast: reaching that code path at all on this backend would hang the boot with no log output, not produce a diagnosable error.
