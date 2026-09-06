# AGENTS.md — AI Agent Guide

Focus rules only. Full architecture is in `CLAUDE.md` and `.claude/rules/`.

## Repository layout

- Root crate + `migration/` crate + `ee/` module.
- `src/models/` owns queries; `src/controllers/` maps HTTP; `services/mcp/` owns MCP tools.
- `ee/` is composition root only in `src/app.rs`.

## Architecture

- Loco + SeaORM. One migration file: `migration/src/m20260829_000000_initial_schema.rs`.
- Two pools: `identity` (control plane) and `tenant` (RLS-scoped). See `src/db.rs`.
- Entity API is the default path. Raw SQL only where SeaORM cannot express it (JSONB containment, pgvector, advisory locks).

## SQLite

- Single-tenant only. No RLS. `src/db.rs::require_min_sqlite_connections(2)`.
- Vector search via sqlite-vec. Id generation via `before_save` or explicit `db::sqlite_generated_id()`.
- Embedding column lives in `content_entity_embeddings` on both backends.

## Error handling

- `YorishiroError` is the primary error type. Use `.internal()` for conversion.
- `YorishiroError::not_found()` for NotFound construction.

## Imports

- `use axum::http::StatusCode;` (never fully-qualified).
- Group: std → external → workspace → crate-internal.

## Testing

- `tests/` is the integration test crate. `tests/requests/mod.rs` owns `close_app_pools`.
- Request tests using `request_with_create_db` **must** call `close_app_pools` before returning.

## Git workflow

- Branch from latest `develop`. Merge commits preferred.
- `cargo check && cargo clippy -- -D warnings && cargo fmt --check` before pushing.
- Every PR must update English + Japanese docs.
