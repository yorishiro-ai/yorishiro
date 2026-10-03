# Loco and SeaORM refactor

The goal is to return the application to Loco 1.2.0 and SeaORM 2.0.4 conventions.
Project-specific code remains only where the framework APIs cannot preserve required behavior.

## Principles

- Keep application-owned model extensions at `src/models/<table>.rs`, matching Loco's generator layout.
- Allow private implementation modules below a large model extension without changing its flat public path.
- Keep generated entities under `src/models/_entities/` and regenerate rather than hand-edit them.
- Put finders and table-owned persistence in models, state transitions on `ActiveModel`, and request parsing and rendering in controllers.
- Use Loco configuration, lifecycle hooks, queues, workers, tasks, initializers, and test harness before adding custom infrastructure.
- Use SeaORM entity and query APIs before raw SQL.
- Keep raw SQL for RLS and session primitives, security-definer authentication, advisory locks, backend extensions, unsupported query shapes, and writes whose correctness depends on one statement.
- Keep the identity and tenant pools because Loco does not expose the connection lifecycle needed for tenant RLS reset.
- Keep the `ee/` licence boundary and compose it from `src/app.rs`.

## Work queue

### P0: restore authoritative rules

- [x] Add `.agents/rules/loco-architecture.md` as the project-specific authority.
- [x] Update `AGENTS.md` to describe the canonical flat model layout and real test commands.
- [x] Correct stale testing, edition, module, and Git workflow rules.
- [x] Keep `gh-wait` optional and retain the repository-specific CI workflow table.

### P0: restore the Loco model layout

- [x] Place every application model at `src/models/<table>.rs` and declare it through `src/models/mod.rs`.
- [x] Keep `entity_entities` and `tenancy` private implementation directories below their owning flat model extensions.
- [x] Remove area modules, zero-byte sentinels, and `#[path]` compatibility declarations.
- [x] Move production imports to flat application-owned model paths, then rewrite test imports last.
- [x] Run entity regeneration and prove generated extensions are not overwritten.
- [x] Preserve `_entities`, `pagination`, and justified multi-table/query modules.

### P0: use SeaORM consistently

- [x] Fix partial `ActiveModel` updates so `updated_at` is stamped through `db::stamped_updated_at`.
- [x] Replace duplicate reindex candidate SQL with one model finder.
- [x] Replace the simple PostgreSQL row-lock SQL with `QuerySelect::lock_exclusive`.
- [x] Move audited table-owned queries out of controllers, services, and scheduled tasks.
- [x] Keep documented raw SQL exceptions unchanged.

### P1: reduce the composition root

- [x] Move Loco dispatcher implementations out of `src/app.rs`; retain the EE-aware queue policy at the composition boundary.
- [x] Move community route composition and inventory assembly into `src/app/routes.rs` without hiding explicit registration.
- [x] Move seed composition into `src/app/seed.rs`.
- [x] Keep `src/app.rs` as the `Hooks` implementation and sole base-to-EE composition boundary.

### P1: use Loco configuration and lifecycle

- [x] Replace application `std::env` reads with validated typed settings loaded once.
- [x] Keep direct environment access only in configuration/bootstrap code with an explicit reason.
- [x] Replace unmanaged durable work with Loco workers and await finite startup scans; retain only the shutdown-managed load monitor.
- [x] Centralize rate limiter construction and settings; do not claim distributed guarantees without an atomic shared backend.

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

- [x] Use Loco's `config/{environment}.yaml` loader as the configuration source of truth.
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
