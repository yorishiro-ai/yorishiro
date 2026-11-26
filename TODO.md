# Work plan for #642 to #645 and database independence

This is a working document for the `refactor/iam-foundation` branch.
Delete it before the pull request is opened.

## Decisions

- Breaking changes are allowed. Migrations are rewritten in place and numbers stay contiguous.
- Resource names are YRN, not ARN. The bearer key prefix stays `ysr_`.
- The `tenant` term stays. Headers stay lowercase `x-workspace-id` and `x-tenant-id`.
- Application code creates timestamps and ids. The database does not default them.
- Tenant isolation stays in SQL: RLS, GRANT, the `yorishiro_app` role, `authenticate_access_key`, tenant pool `SET ROLE`, and the search `SET LOCAL`.
- Tests run only after all changes are made. After a migration change, run `make entities`.
- The real-binary Python contract suite and its CI jobs are removed; so are migration-content tests.
- No migration tests are added. Moved behaviour is verified through model operations.
- No push, pull request, or merge without authorisation.

## YRN format

| Target | Form |
|---|---|
| Tenant | `yrn:tenant:<tenant>` |
| Workspace | `yrn:workspace:<tenant>:<workspace_id>` |
| All workspaces | `yrn:workspace:<tenant>:*` |
| User | `yrn:iam:<tenant>:user/<name>` |
| Tenant root | `yrn:iam:<tenant>:root` |
| Group | `yrn:iam:<tenant>:group/<name>` |
| Tenant policy | `yrn:iam:<tenant>:policy/<name>` |
| Managed policy | `yrn:iam::policy/<Name>` |

An empty tenant field means a managed policy.
A resource kind is written only when a service has more than one kind.
Parsing lives in one place and fixes the field count per service.

## Steps

### 1. Safety net before removal
- [ ] Tests for `embedding_sync_token`, embedding revision, SQLite FTS rows, template origin detachment, fork integrity, and timestamps.
- [ ] Cover every write path: create, update, `update_if_unchanged`, fill-defaults, undo, delete, workspace delete (cascade), tenant retirement, embedding key save and delete.
- [ ] Keep `query_only` embedding key updates from raising the revision.

### 2. Time and ids in the application
- [x] Remove `DEFAULT now()`, `CURRENT_TIMESTAMP` and `uuidv7()` defaults from migrations.
- [x] Set `created_at`, `updated_at` and ids in `before_save` and in bulk update values.
- [x] Bind application time in `system_maintenance`, `queue_job_dispatch_outbox`, `authenticate`, and `iam_groups`.

### 3. Loco connection pool
- [x] Delete `replace_app_pool`, the SQLite `current_timestamp` FFI, and the identity pool TimeZone hooks.
- [x] Keep the tenant pool `SET ROLE`.
- [ ] Delete `cli_create_app_keeps_migrations_on_the_serving_in_memory_database`.

### 4. Entity behaviour in models
- [x] `embedding_sync_token` assignment.
- [x] Workspace embedding revision increments.
- [x] SQLite FTS synchronisation.
- [x] Remove the matching triggers and functions from migrations.

### 5. Credentials and templates
- [x] Raise the revision on embedding key save and delete.
- [x] Detach schema origins on template delete and tenant retirement, on the identity pool.

### 6. Schema fork integrity (kept in SQL)
- [x] Decided to keep the integrity triggers and the fork read functions.
- They are `SECURITY DEFINER` because they read rows of another workspace, which the tenant role cannot see under RLS.
- They are not recursive, so a recursive CTE cannot replace them.
- They belong to the tenant isolation boundary, which stays in SQL.

### 7. Fork read functions (kept in SQL)
- [x] See step 6.

### 8. Remaining DDL through sea-query
- [x] GIN and trigram expression indexes with operator classes.
- [x] `vector` column and hash partitions.
- [ ] One shared definition of `normalize(data::text, nfkc)`.
- [ ] Rewrite the `indexes.rs` catalog query.
- [ ] Route remaining value-only backend branches through `by_backend`.

### 9. YRN and managed policies
- [x] Rename ARN to YRN in code, errors, schema text, and tests.
- [x] Rename the caller identity field `arn` to `yrn`.
- [x] Define managed policies in code and attach by YRN.
- [x] Remove per-tenant managed policy rows.

### 10. Workspace policies
- [x] Add `WorkspaceViewerAccess` and `WorkspaceContributorAccess`.
- [x] Document scoping to one workspace through the Resource element (managed policies apply to every workspace; scope with a custom policy).

### 11. Key model
- [x] Remove the key `audit` column and field.
- [ ] Remove workspace-bound keys and `tenant_scoped`.
- [ ] Select the workspace per request.
- [ ] Update `authenticate_access_key`.

### 12. Naming
- [ ] Fix remaining response fields and stale descriptions.
- [ ] Cross-check with the #645 mapping.

### 13. Inference
- [ ] Merge `reject` into `discard`, with status, routes, tools, and constraints.
- [ ] Drop the naming test exception.

### 14. Verification
- [ ] `make check-all check-all-ee`.
- [ ] SQLite and PostgreSQL full suites for CE and EE.

### 15. Notes
- [ ] Update the #644 and #645 checklists and prepare breaking-change notes without publishing.
