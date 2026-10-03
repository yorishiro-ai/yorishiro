# Source Layout

This page describes how the source tree follows Loco 1.2.0's generator layout, and why the few directories Loco does not generate exist.

## Layers

Every entry point authenticates, parses input, calls a model operation, and renders the result.
None of them owns domain logic or builds ordinary queries.

| Directory | Role | Laravel counterpart |
| --- | --- | --- |
| `src/controllers/` | Entry points: REST handlers, MCP tools (`controllers/mcp/`), request middleware (`controllers/middleware/`), extractors | Controllers, Middleware |
| `src/tasks/` | Entry point: the command line | Console commands |
| `src/dtos/` | Request and response transport types, one file per controller | Resources, Form requests |
| `src/models/` | One extension per table (`<table>.rs`), generated `_entities/`, and cross-table models | Eloquent models |
| `src/workers/` | Background jobs and the queue-lifecycle protocol they share | Jobs |
| `src/initializers/` | Work that runs for the lifetime of the process | Service providers |
| `src/data/` | Typed settings, configuration loading, and built-in data | Config |
| `src/fixtures/` | Seed data | Seeders |
| `src/services/` | External clients only (embedding providers), which Loco has no place for | Services |

`ee/` is an overlay on `src/`.
It mirrors the folder hierarchy and file names of the community edition, so a community path tells you where its enterprise counterpart lives.
Its `services/` directory follows the same restriction: only outbound LLM and OAuth clients remain there; enterprise authentication, licence middleware, plan data, and persistence live under their matching controller, data, and model paths.

## Generator Boundary

Migrations under `migration/src/` define the database schema and are hand-written.
`cargo loco db entities` generates SeaORM entities under `src/models/_entities/`.
The generated directory stays at that path and must not be edited by hand.
Loco 1.2.0 hard-codes `src/models/_entities` as its output directory.

The application-owned extension for a table lives at `src/models/<table>.rs`, which is where the generator writes a missing one.
A large model may keep private implementation modules under `src/models/<table>/`, declared by `src/models/<table>.rs`.
Finders and persistence owned by one table live in that extension, with no repository layer in between.
Models that span several tables, such as `tenancy`, `search`, and `entity_embeddings`, live beside the table models.

The checked-in `Makefile` target `entities` runs migrations and `cargo loco db entities` against disposable PostgreSQL.
CI verifies that regeneration produces at least 10 entity files and compares the result with the checked-in tree.
The three pgvector entity files are restored before that comparison because SeaORM codegen omits their required `ActiveModelBehavior` implementation.

## Secret-bearing Generated Models

SeaORM 2.0.4 does not provide a per-table option to replace the generated `Debug` derive.
The `make entities` target therefore runs `scripts/harden_generated_entities.py` after code generation.
That post-processor uses an explicit suspicious-name policy: exact credential names and qualified credential suffixes such as `_api_token`, `_service_key`, `_password`, and `_authorization` are protected, while generic names such as `key` and `*_hash` are not.
It removes `Debug` from the model derive and adds `serde(skip_serializing)` to every suspicious field.
If a suspicious field appears in an unsupported generated shape, the post-processor fails instead of silently leaving the field exposed.
The application-owned modules `src/models/workspace_llm_keys.rs` and `src/models/workspace_embedding_keys.rs` provide safe `Debug` implementations that omit `api_key` while preserving the generated query and write types.
Do not edit `_entities` by hand, and keep the post-generation hardening step in the entity workflow when changing the generator command.

## Static Data

Built-in template JSON assets live under `data/templates/` and are embedded by `src/data/templates.rs`.
They are schema definitions, not generated entities, so they are reviewed and tested as source assets.
