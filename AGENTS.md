# Knowledge

## Repository overview

Yorishiro uses [Loco](https://loco.rs) 1.3.1.

## Repository layout

Root crate + `migration/` crate + `ee/` module, assembled in `src/lib.rs`.
`ee/` is an overlay on `src/`: it mirrors CE's folder hierarchy and file names, so a CE path tells you where its enterprise counterpart lives.
CE therefore follows Loco's generator layout exactly; a directory Loco does not generate needs a written reason below.

| Path | Role |
|---|---|
| `src/controllers/` | REST and MCP handlers, request middleware, and extractors |
| `src/tasks/` | CLI commands |
| `src/dtos/` | Request and response types (`dtos/<plural>.rs`) |
| `src/models/` | Table modules, generated entities, and cross-table operations |
| `src/workers/` | Background jobs and runtime routing |
| `src/initializers/` | Process-lifetime wiring |
| `src/data/` | Typed settings and static data |
| `src/services/` | External clients only, such as embedding providers |
| `src/app.rs` | Community hooks and edition-neutral wiring |
| `src/lib.rs` | Crate assembly and sole `Hooks` implementation |
| `.github/packaging/` | Package definitions, service units, install hooks, and checks |

Every entry point (REST, MCP, CLI) authenticates, parses input, calls model operations, and renders.
None of them owns domain logic or builds ordinary queries.

### Edition boundary

`src/` is the community base; only `src/lib.rs` assembles the enterprise overlay.
Follow the [edition rules](.agents/rules/editions.md) when changing base or overlay wiring.

The base still names a few things only the overlay uses, because they belong to the shared schema and to stored data, not to behaviour:
- the table modules for overlay-only tables (see Model ownership);
- the `Action` names in `models/iam/policy.rs`: a policy document is stored data, and a tenant that changes edition must still parse every statement it holds;
- tenant retirement, which deletes the rows of every tenant-scoped table in the schema, including the schema-fork tables, so a retired tenant leaves nothing behind.

None of these is a branch: the base never calls into the overlay for them.

### Model ownership

`src/models/mod.rs` groups the table modules by domain (tenants, IAM, schemas, entities, queue, billing); look a table up there first.
`src/models/iam/access.rs` owns cross-table authorization decisions shared by REST, MCP, CLI and other model callers.
Credential verification and table persistence remain in their table-owned models; access is not a repository or external service layer.

Cross-table IAM operations live under `src/models/iam/`, and the shared JSONL import and export format lives under `src/models/transfer/`.

- One table has one application-owned extension at `src/models/<table>.rs`, matching Loco's generator layout.
- Regenerate `src/models/_entities/` with `make entities`; never edit it by hand.
- Finders and table-owned persistence live in that table's extension module. Do not add a repository layer.
- `src/models/mod.rs` is the module entry point; keep domain logic out of it.
- A large model may keep private implementation modules under `src/models/<table>/`, declared by `src/models/<table>.rs`.
- Do not add `#[path]` bridges or zero-byte generator sentinels.
- Adding a table means writing a migration under `migration/src/`, running `make entities`, and adding `src/models/<table>.rs` when business logic is needed.

### Not a model

- `migration/src/` and `src/db.rs` are database concerns, not model concerns. They stay outside `models/`.
- Built-in template JSON is static data: `src/data/templates.rs` embeds `src/templates/*.json`.
- `src/db.rs` handles connection pooling, two-pool architecture (identity pool for control plane, tenant pool for RLS-scoped requests), and session primitives (`SET ROLE`, `RESET`, `set_config`).

### Raw SQL

SeaORM is the default. Raw SQL requires an explicit reason:
- JSONB containment (`data @> filter`)
- Vector similarity search with pgvector or sqlite-vec
- Advisory locks
- Query shapes that SeaORM cannot express without obscuring their semantics
- Writes whose correctness depends on executing as a single statement
- Measured performance requirements documented beside the query

Examples:
- `src/db.rs`: connection primitives (`SET ROLE`, `RESET`, advisory locks)
- `src/models/access_keys/authenticate.rs`: `authenticate_access_key($1)`, a SECURITY DEFINER function that bypasses RLS
- `src/models/entity_embeddings/` and `src/tasks/resync_embeddings.rs`: pgvector `embedding` column operations

### MCP placement

MCP is an entry point, so it is a controller: `src/controllers/mcp/` holds one file per tool group (the `#[tool_router]` implementations), `server.rs` (`YorishiroMcpServer`), `policy.rs` (which tools a request may see), `authorize.rs` (bearer key and scope checks) and, in `mod.rs`, the router composition and transport mount.
Adding a tool group touches `mod.rs` and its own file.
Each tool authenticates, parses its arguments, calls a model operation, and renders a tool result, exactly like a REST handler.

## Architecture

- Loco + SeaORM + sea-query
- Two connection pools: `identity` (control plane, migration-role) and `tenant` (RLS-scoped). See `src/db.rs`.
- Entity API is the default path for reads and writes.

## Database backends

- SQLite supports a single tenant without RLS.
- `src/db.rs::require_min_sqlite_connections(2)` requires at least 2 connections.
- Vector search uses sqlite-vec (BLOB storage).
- Models create ids and timestamps on both backends through `db::generated_id` and `db::stamped_*`; no database default or trigger supplies them.
- The embedding sync token and the template origin detach are application code, not triggers.
- The embedding column lives in `entity_embeddings` on both backends, with a `(workspace_id, entity_id)` primary key.
- PostgreSQL uses 64 workspace hash partitions, tenant RLS, and scheduled workspace-specific HNSW promotion starting at 2,000 vectors.
- Each promoted workspace has one partial pgvector HNSW index: up to 2,000 dimensions on `embedding::vector(N)`, 2,001 through 4,000 on the normalized `halfvec(N)`, and 4,001 through 4,096 on the normalized first 4,000 dimensions as `halfvec(4000)`.
- Every search takes at most 1,000 index candidates and reranks them in original full-precision coordinates; the `embedding_index_maintenance` scheduler job builds and retires the indexes.
- Search ranks by cosine distance on both backends.
- PostgreSQL HNSW can select different candidates from SQLite's exact scan.

### Database-owned behaviour

PostgreSQL owns RLS policies, per-table grants, `authenticate_access_key`, partitions, and fork read functions.
Both backends enforce fork integrity with triggers.

### Migration helpers (`migration/src/helpers.rs`)

- `use_transaction()` forces a transaction on every backend.
- `uuidv7_pk(manager)` is the `id` column: `UUID PRIMARY KEY` on PG, `BLOB PRIMARY KEY` on SQLite, with no default.
- `uuid_col(manager, name)` is `UUID` on PG, `BLOB` on SQLite.
- `json_col(manager, name)` is `JSONB` on PG, `TEXT` on SQLite.
- `ts_tz_col(manager, name)` is `TIMESTAMPTZ` on PG, `TEXT` on SQLite.
- `created_at(manager)` and `timestamps(manager)` build the application-stamped timestamp columns, without defaults.
- `create_table_with_checks()` creates a table with CHECK constraints on both backends.
- `enable_rls_with_policy()` enables RLS with a strict or lenient policy. No-op on SQLite.
- `grant()` is a per-table GRANT to `yorishiro_app`. No-op on SQLite.
- `pg_only(sql)` runs PostgreSQL-only raw SQL.
- `sqlite_only(sql)` runs SQLite-only raw SQL.
- `by_backend(manager, postgres, sqlite)` picks a value per backend.

## Error handling

- `YorishiroError` is the primary error type. Use `.internal()` for conversion.
- `YorishiroError::not_found()` for NotFound construction.
- `ResultExt` trait provides `.internal()` which wraps into `YorishiroError::Internal`.
- `into_http_parts()` converts errors to HTTP response components.

## Imports

- `use axum::http::StatusCode;` (never fully-qualified).
- Group order: std → external → workspace → crate-internal.

## Rules

Read and follow the applicable rule files before making changes:

- [Editions](.agents/rules/editions.md): edition boundaries, licensing, and overlay wiring.
- [Testing](.agents/rules/testing.md): test commands, backend requirements, and request teardown.
- [Git workflow](.agents/rules/git-workflow.md): branches, PRs, CI checks, and releases.
- [Prose](.agents/rules/prose.md): Markdown, comments, commit messages, and PR text.
