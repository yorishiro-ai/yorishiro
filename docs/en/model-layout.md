# Model Layout

This page describes the first phase of Issue #472.

## Generator Boundary

Migrations under `migration/src/` define the database schema and are hand-written.
`cargo loco db entities` generates SeaORM entities under `src/models/_entities/`.
The generated directory stays at that path and must not be edited by hand.
The Loco 1.1.0 generator hard-codes `src/models/_entities` as its output directory.
It also reads `src/models/mod.rs` and creates a hand-written extension at that root when one is missing.
The root model files therefore remain as compatibility markers while their implementation sources live in feature areas.
Controllers, tasks, workers, and hand-written model logic remain application code and are not generated.

The checked-in `Makefile` target `entities` runs migrations and `cargo loco db entities` against disposable PostgreSQL.
CI verifies that regeneration produces at least 10 entity files and compares the result with the checked-in tree.
The three pgvector entity files are restored before that comparison because SeaORM codegen omits their required `ActiveModelBehavior` implementation.
On 2026-09-28, a direct run against the existing disposable PostgreSQL container generated 31 files under `src/models/_entities`.
All generated files were byte-identical to the baseline after restoring those three documented pgvector exceptions.

## Feature-Area Map

Every moved hand-written model module has exactly one area.
`pagination.rs` remains intentionally rooted because it is shared model infrastructure rather than a table model.
The root marker files preserve the old `crate::models::<module>` paths for existing callers.

| Area | Moved modules | New implementation path |
| --- | --- | --- |
| Content | `entity_column_preferences`, `entity_embeddings`, `entity_embeddings_1024`, `entity_embeddings_1536`, `entity_embeddings_768`, `entity_entities`, `entity_relations`, `entity_snapshots`, `export`, `import`, `recall`, `schema_schemas`, `search`, `workspace_schema_fork_heads`, `workspace_schema_forks` | `src/models/content/` |
| Identity | `api_key_audit_log`, `api_keys`, `tenancy`, `tenant_memberships`, `tenant_tenants`, `user_users`, `workspace_embedding_keys`, `workspace_invites`, `workspace_llm_keys`, `workspace_workspaces` | `src/models/identity/` |
| Templates | `template_reviews`, `template_templates`, `template_versions` | `src/models/templates/` |
| System | `compute_credit_ledger`, `inference_jobs`, `stripe_events`, `system_maintenance`, `tenant_billing`, `tenant_reindex_schedules`, `workspace_worker_classes` | `src/models/system/` |

Intentionally rooted model modules are `pagination.rs` and the compatibility marker files in `src/models/`.
`_entities/` is intentionally rooted and generated, not a feature-area model implementation.
The nested `src/models/content/entity_entities/` directory is an implementation detail of the content model and still represents one table model.
