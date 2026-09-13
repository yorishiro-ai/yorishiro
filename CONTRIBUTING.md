# Contributing

**English** | [日本語](docs/ja/contributing.md)

## Code layout

| Location | Holds |
|---|---|
| `src/models/` | Record shapes, input DTOs, and queries |
| `src/controllers/` | Request handlers |
| `src/services/` | Auth, embedding, MCP |
| `migration/src/` | Schema migrations |
| `src/tasks/` | Admin and one-off commands |
| `ee/` | Enterprise edition (mirrors `models/`, `controllers/`, `services/`) |

- `src/models/_entities/` is auto-generated. Never edit by hand.
- Business logic goes in `src/models/<table>.rs` next to the generated entity.
- Raw SQL is only where SeaORM cannot express it (JSONB containment, pgvector, advisory locks).
- `src/db.rs` handles connections, not tables.

## Adding a table

1. Write a migration under `migration/src/`.
2. Run `make entities` to regenerate `_entities/`.
3. Add a module under `src/models/`.

## Tests

```console
$ make test
```

Request tests must call `close_app_pools` before returning. See `tests/requests/mod.rs` for the pattern.

Metaschema validation includes property-based tests for arbitrary schema-shaped definitions and rejection of empty `entity_types`.

## Before you push

```console
$ make check          # cargo check --workspace
$ make clippy         # cargo clippy --workspace --tests -- -D warnings
$ make fmt-check      # cargo fmt --check
$ make test           # cargo test --workspace
```

Tests need PostgreSQL with `vector` and `pg_trgm` in `template1`, and a non-superuser role.

## Git workflow

Branch from `develop`. Merge commits preferred. Every PR updates English + Japanese docs.

## Rules

Rules for code style live at `.claude/rules/`.
