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
| `src/app.rs` | The community wiring, one function per Loco `Hooks` method, and the types an edition extends it with | Application bootstrap |
| `edition/` | The composition root: the only `Hooks` implementation, outside `src/` | Service container |

`ee/` is an overlay on `src/`.
It mirrors the folder hierarchy and file names of the community edition, so a community path tells you where its enterprise counterpart lives.
Its `services/` directory follows the same restriction: only outbound LLM and OAuth clients remain there; enterprise authentication, licence middleware, plan data, and persistence live under their matching controller, data, and model paths.

## Edition Composition

`src/` never refers to the enterprise overlay: no `cfg(feature = "enterprise")`, no `crate::ee`, and no overlay names.
`scripts/check_edition_boundary.py` enforces this over every Rust file under `src/`, and it runs in `make check-all`, `make check-all-ee`, and CI.
It also rejects `edition` and `ee` as identifiers anywhere in `src/` except that `src/lib.rs` may declare and re-export the composition root, so an alias or a path split across spaces or lines cannot get around it.

The crate has one `Hooks` implementation, `edition::App` in `edition/app.rs`.
Each hook calls the matching function in `src/app.rs` and then lets the active edition add to the result.
The active edition is `ee/app.rs` when the `enterprise` feature is on and `edition/community.rs` otherwise.
Both expose the same functions, so a signature mismatch fails the build of whichever edition fell behind.
There is no trait between them, because there is one overlay and no runtime choice to make.

`src/` exposes small, edition-neutral inputs for the overlay to use:

| Input | What an edition does with it |
| --- | --- |
| `workers::registry::WorkerRegistry` | Adds a worker type with `register`. Its tags and queue come from the worker type itself. |
| `app::RouteMounts` | Mounts route groups and their OpenAPI documents, and names credential paths that share the per-IP rate limit. |
| `controllers::mcp::McpToolSet` | Adds MCP tools and the policy that decides which of them a request may use. |
| `shared_store` seams | Replace a rule at runtime by inserting its own `Arc<dyn Trait>`, such as the authenticator or the queue policy. |

Worker tags, the Redis queue configuration, and Loco worker registration are all derived from the one composed `WorkerRegistry`, so `yorishiro worker-tags` cannot disagree with what the process registers.

A table keeps its module under `src/models/` even when only the overlay uses it.
The migrations and generated entities are one shared schema, and every generated entity needs an `ActiveModelBehavior` implementation (and, for secret-bearing ones, a redacting `Debug`) in every build.
The behaviour the overlay owns for that table lives in the matching file under `ee/models/`.

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
The application-owned modules for the two credential tables, `workspace_llm_keys.rs` and `workspace_embedding_keys.rs` under `src/models/`, provide safe `Debug` implementations that omit `api_key` while preserving the generated query and write types.
Do not edit `_entities` by hand, and keep the post-generation hardening step in the entity workflow when changing the generator command.

## Static Data

Built-in template JSON assets live under `data/templates/` and are embedded by `src/data/templates.rs`.
They are schema definitions, not generated entities, so they are reviewed and tested as source assets.
