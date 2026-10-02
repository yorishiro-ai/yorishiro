# Loco and SeaORM refactor

The goal is to return the application to Loco 1.2.0 and SeaORM 2.0.4 conventions.
Project-specific code remains only where the framework APIs cannot preserve required behavior.

## Principles

- Group canonical model extensions under `content`, `identity`, `system`, and `templates` using normal Rust modules.
- Preserve stable flat imports with re-exports while production callers move to canonical area paths.
- Keep generated entities under `src/models/_entities/` and regenerate rather than hand-edit them.
- Put finders and table-owned persistence in models, state transitions on `ActiveModel`, and request parsing and rendering in controllers.
- Use Loco configuration, lifecycle hooks, queues, workers, tasks, initializers, and test harness before adding custom infrastructure.
- Use SeaORM entity and query APIs before raw SQL.
- Keep raw SQL for RLS and session primitives, security-definer authentication, advisory locks, backend extensions, unsupported query shapes, and writes whose correctness depends on one statement.
- Keep the identity and tenant pools because Loco does not expose the connection lifecycle needed for tenant RLS reset.
- Keep the `ee/` licence boundary and compose it from `src/app.rs`.

## Work queue

### P0: restore authoritative rules

- [ ] Add `.agents/rules/loco-architecture.md` as the project-specific authority.
- [ ] Update `AGENTS.md` to describe the canonical flat model layout and real test commands.
- [ ] Correct stale testing, edition, module, and Git workflow rules.
- [ ] Keep `gh-wait` optional and retain the repository-specific CI workflow table.

### P0: finish the grouped model layout

- [ ] Declare every grouped model through its area's `mod.rs`.
- [ ] Keep `entity_entities` and `tenancy` private implementation directories inside their owning areas.
- [ ] Remove marker-only area modules, zero-byte sentinels, and `#[path]` compatibility declarations.
- [ ] Move production imports to canonical area paths, then rewrite test imports last.
- [ ] Run entity regeneration and prove generated extensions are not overwritten.
- [ ] Preserve `_entities`, `pagination`, and justified multi-table/query modules.

### P0: use SeaORM consistently

- [ ] Fix partial `ActiveModel` updates so `updated_at` is stamped through `db::stamped_updated_at`.
- [ ] Replace duplicate reindex candidate SQL with one model finder.
- [ ] Replace the simple PostgreSQL row-lock SQL with `QuerySelect::lock_exclusive`.
- [ ] Move table-owned queries out of controllers, services, and scheduled tasks one operation at a time.
- [ ] Keep documented raw SQL exceptions unchanged.

### P1: reduce the composition root

- [ ] Move Loco dispatcher implementations and queue policy out of `src/app.rs`.
- [ ] Move route composition and inventory assembly into `src/app/routes.rs` without hiding explicit registration.
- [ ] Move seed composition into `src/app/seed.rs`.
- [ ] Keep `src/app.rs` as the `Hooks` implementation and sole base-to-EE composition boundary.

### P1: use Loco configuration and lifecycle

- [ ] Replace application `std::env` reads with validated typed settings loaded once.
- [ ] Keep direct environment access only in configuration/bootstrap code with an explicit reason.
- [ ] Replace unmanaged process-lifetime `tokio::spawn` work with initializers/hooks and `on_shutdown`, or durable workers/tasks.
- [ ] Centralize rate limiter construction and settings; do not claim distributed guarantees without an atomic shared backend.

### P1: remove duplicated HTTP contracts

- [ ] Derive OpenAPI schemas conditionally on canonical wire DTOs.
- [ ] Delete schema-only mirrors where wire shape is identical.
- [ ] Retain dedicated OpenAPI types only for intentional contract differences.
- [ ] Move HTTP request DTOs out of EE model modules.

### P1: delete speculative and duplicate abstractions

- [ ] Remove the unused WASM offload hook and recommendation foundation.
- [ ] Collapse delegate-only test dispatcher adapters where the production trait is sufficient.
- [ ] Preserve test seams that cover real concurrency and failure behavior.

### P1: trim dependencies and features

- [ ] Remove unused `insta`, `rstest`, and migration-crate `loco-rs` dependencies.
- [ ] Replace direct `url::Url` use with the already available `reqwest::Url`, then remove the direct optional dependency.
- [ ] Disable unnecessary SQLx defaults after RLS, migration, and backend verification.
- [ ] Evaluate reduced Reqwest, JSON Schema, tokenizer, and test-library features with behavior tests.
- [ ] Do not remove direct SQLx, Axum, async-trait, SQLite FFI/vector, embedding, MCP, or OpenAPI dependencies.

## Deferred compatibility removal

- [ ] Remove the legacy `config/{environment}.yaml` loader only at its documented compatibility boundary.
- [ ] Re-evaluate duplicate `jsonwebtoken` major versions as a separate security-sensitive change.
- [ ] Change pagination semantics only after measured offset-pagination cost justifies an API change.

## Verification after each boundary

- [ ] `make check-all`
- [ ] `cargo check --locked --no-default-features`
- [ ] `cargo check --locked --features openapi`
- [ ] `make test-postgres`
- [ ] `make test-sqlite`
- [ ] `make entities` followed by a clean generated-tree diff
- [ ] Route inventory, OpenAPI, task, worker, and migration registration tests
