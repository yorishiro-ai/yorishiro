# REST API & MCP Tools

**English** | [日本語](ja/api.md)

## REST API

Key endpoints (see the Swagger UI at `/docs` for the full list and details):

```console
# Register a schema (schema scope)
$ curl -X POST localhost:8080/api/schemas \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d @templates/task-management.json

# Create an entity (write scope)
$ curl -X POST localhost:8080/api/entities \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d '{"schema_name":"task-management","entity_type":"task","data":{"title":"Buy milk"}}'

# Vector similarity search, combined with a structured filter (read scope)
$ curl "localhost:8080/api/search?query_text=shopping&filter=%7B%22status%22%3A%22active%22%7D" \
    -H "Authorization: Bearer $YSR_KEY"

# A new workspace has no schema yet, so entity writes are refused with a 422 until one exists
# ("create a schema first: POST /api/schemas..."). Creating the first schema lifts this and
# moves the workspace's `status` from schema_pending to active.

# Retire a relation without deleting it: traversal stops following it, the record stays
# (write scope). Statuses are active, deprecated and archived.
$ curl -X PUT "localhost:8080/api/relations/$RELATION_ID/status" \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d '{"status": "deprecated"}'

# List only the relations in one state; omit `status` to list every state (read scope)
$ curl "localhost:8080/api/relations?status=active" -H "Authorization: Bearer $YSR_KEY"

# How an entity stands against the active version of its schema (read scope). Entities are
# migrated lazily, so one written earlier simply lacks fields added since -- this tells that
# apart from a field its author left blank.
$ curl "localhost:8080/api/entities/$ENTITY_ID/drift" -H "Authorization: Bearer $YSR_KEY"

# Entity plus its relations and connected neighbors in one call (read scope)
$ curl "localhost:8080/api/entities/$ENTITY_ID/context" -H "Authorization: Bearer $YSR_KEY"

# JSON Lines export: every schema version in the tenant, plus this workspace's entities and
# relations (read scope)
$ curl "localhost:8080/api/export.jsonl" -H "Authorization: Bearer $YSR_KEY"

# Import the same JSON Lines format back in, as a single transaction (schema scope,
# since importing schemas is itself a schema-scope-only operation)
$ curl -X POST localhost:8080/api/import.jsonl -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/x-ndjson" --data-binary @export.jsonl
```

`GET /api/entities` also accepts a `filter` query parameter (a JSON object matched with JSONB containment, e.g. `filter={"status":"active"}`) and a `schema_version` query parameter, and `POST /api/schemas` accepts either an inline definition or `{"template_id": "..."}`.

`template_id` takes both kinds of template, so a caller holding an id does not have to know which kind it is:

| Form | Resolves against | Listed by |
|---|---|---|
| `"task-management"` | The built-in templates compiled into the binary | `GET /api/templates` |
| A UUID | The tenant's own template library | `GET /api/template-library` |

Parsing decides which: a UUID is looked up only in the library, anything else only among the built-ins. A library template belonging to another tenant answers `404` -- the same answer as one that does not exist, so a caller cannot confirm it exists from the difference.

`schema_version` restricts results to entities created against that version of the schema. An entity records the version it was written against and keeps it when a newer version is created, so this returns the entities a given version produced -- not the entities that would validate against it today.

All request bodies are capped at 2 MiB (`413 Payload Too Large` beyond that) -- relevant to `POST /api/import.jsonl` for a large export.

### `GET /api/templates/{id}`

Returns the full definition of a single built-in template by its ID (e.g. `general-notes`) as a `MetaSchemaDefinition` JSON object -- the same structure `POST /api/schemas` accepts.

### Template library

Separate from the built-in templates above (`/api/templates`, read-only, bundled with the server), each tenant also has a DB-backed template library it can create, edit, and fork templates in:

| Endpoint | Scope | Description |
|---|---|---|
| `GET /api/template-library` | any valid API key | List templates visible to the caller's tenant (own plus any community-visible ones) |
| `GET /api/template-library/{id}` | any valid API key | Fetch a single template by ID |
| `POST /api/template-library` | owner/admin | Create a template |
| `PUT /api/template-library/{id}` | owner/admin | Update a template |
| `DELETE /api/template-library/{id}` | owner/admin | Delete a template |
| `POST /api/template-library/{id}/fork` | owner/admin | Fork an existing template into a new one |

The read endpoints only require a valid API key for the tenant (no tenant-membership check beyond that). As with member/workspace management, the write endpoints are additionally gated on the caller's tenant role (owner/admin), independent of their key's own scope.

A fork is an independent copy that only records which template it came from, so deleting a template that others were forked from succeeds -- the forks themselves stay intact and usable, and just lose the pointer back to the deleted original.

### Template marketplace

Tenants share templates with each other. `identity.templates` already carries `visibility` (`tenant` | `community`) and `fork_of`; the marketplace adds what makes a shared template safe to consume -- published versions, and what other tenants thought of them.

| Endpoint | Scope | Purpose |
|---|---|---|
| `GET /api/marketplace` | any valid API key | Community-visible templates across every tenant, with the latest stable version and review aggregates |
| `GET /api/marketplace/{id}/versions` | any valid API key | Published versions, newest first. Your own drafts are included only for templates your tenant owns |
| `POST /api/marketplace/{id}/versions` | any valid API key | Publish the next version of your own template (`definition`, optional `changelog`, `status` of `draft`/`pre`/`stable`) |
| `GET /api/marketplace/{id}/reviews` | any valid API key | Reviews of a template you can see |
| `POST /api/marketplace/{id}/reviews` | any valid API key | Leave or replace your tenant's review (`rating` 1-5, optional `comment`) |
| `POST /api/marketplace/{id}/fork?version=N` | any valid API key | Copy a published version into your own library. Omitting `version` takes the latest `stable` |
| `PUT /api/marketplace/{id}/visibility` | any valid API key | List your own template in the marketplace, or take it back down |

A version number is assigned by the server, incrementing per template. Letting a client choose it invites gaps and collisions in a sequence other tenants read as history.

**A draft is visible only to the tenant that owns it**, is never forkable, and keeps a template out of the listing entirely until something non-draft is published -- a marketplace entry that 404s on install is worse than a shorter list. A forked copy lands **private** in your own library: republishing someone else's work under your name is a decision, not a default.

Acting on a template your tenant does not own answers `404`, not `403`. A caller that cannot act on a template should not be able to confirm it exists from the difference.

A fork is a template, not yet a schema. Apply it with `POST /api/schemas` and its UUID as `template_id`, exactly as a built-in id is applied.

#### Official listings

The built-in templates are published here too, by `yorishiro-server admin seed-official-templates`. They are ordinary listings -- forkable and reviewable like any other -- attributed to the author `Yorishiro`.

Their publisher is a tenant row with **no members and no workspaces**: `identity.templates.tenant_id` is `NOT NULL` and the marketplace scopes ownership by it, so official listings need an owner. Nobody can log into that tenant, because there is no membership to log in through.

The command is idempotent and meant to run on every deployment: a template already published at the same definition is left alone, and one whose definition changed in a new release publishes a *new version* rather than editing the one tenants already installed.

### Auth & member management

`/auth/signup` and `/auth/login` take no bearer token — their entire purpose is to hand one out. `/setup`/`/setup/status` (see [setup.md](setup.md#first-run-setup)) and the liveness/readiness checks `/up`/`/health` are also unauthenticated. Of those, the four that accept input (`/auth/signup`, `/auth/login`, `/setup`, `/setup/status`) are rate-limited by client IP (`429 Too Many Requests` past the limit; see `YSR_AUTH_RATE_LIMIT_MAX`/`YSR_AUTH_RATE_LIMIT_WINDOW_SECS` in [configuration.md](configuration.md)) -- the health probes `/up`/`/health` are not. See [setup.md](setup.md#signup-login-member-and-workspace-management) for the full invite → signup → login flow.

```console
# Redeem an invite (see `admin create-invite`) to create an account
$ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
    -d '{"invite_token":"...","password":"...","display_name":"..."}'

# Exchange email/password for a freshly issued, role-capped API key. workspace_id is only
# required if the account has access to more than one workspace (a 422 asks for it then).
$ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
    -d '{"email":"...","password":"..."}'

# List / add members of the caller's own tenant (owner/admin only)
$ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/json" -d '{"email":"...","role":"member"}'

# List / create workspaces in the caller's own tenant (listing: any member; creating: owner/admin only)
$ curl localhost:8080/api/workspaces -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/workspaces -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/json" -d '{"name":"staging"}'
```

`POST /api/members` attaches an *existing* account to the caller's tenant. It never creates one -- that's what signup does. Both member-management endpoints are gated on the caller's tenant role (owner/admin), independent of their key's own scope.

Workspace management follows the same rule for `POST`/`DELETE`. Listing and fetching a single workspace's detail, at `GET /api/workspaces/{id}`, are open to any tenant member. The detail response includes a nullable `schema_id` (UUID) -- the schema linked to that workspace. `DELETE` on a tenant's last remaining workspace is rejected with `409 Conflict`.

### Replacing how authentication resolves a key

`authenticate` is this crate's own rule: a presented key resolves to the one workspace recorded on it, and the request's headers do not affect the outcome. A deployment that needs a different rule — a key naming its workspace per request, a key issued by an external identity system, a key carrying a claim this crate has never heard of — implements `yorishiro_core::services::auth::Authenticator` and installs it with `AppState::with_authenticator`.

Every authenticated path resolves through that one value: the `AuthContext`, `Authorized<R>` and `Verified<R>` extractors, and both MCP entry points. Replacing it therefore changes authentication for the whole process rather than for the paths that remembered to ask — a REST route and an MCP tool cannot end up disagreeing about who the caller is.

The implementation receives the request's headers verbatim, so it can read whatever the key itself does not carry. Two obligations it must hold to, because the rest of the system assumes them:

- reject a key it cannot verify with `YorishiroError::Unauthenticated`, rather than returning a context for it
- return a context whose `tenant_id` owns its `workspace_id` — the RLS session variables are set from both, and a mismatched pair produces a session that reads one tenant's workspace under another tenant's policies

Scope is still enforced against whatever context is returned, so replacing authentication is not a way past authorization.

## Unmatched paths (web UI fallback)

Any request path that isn't one of the API routes above falls through to the web UI's static file server, and its behavior depends on whether the path *looks like* a file:

- No file extension (e.g. `/foo`, `/dashboard`, `/schemas/abc`) -- always serves the SPA's `index.html`, `200 OK`. This is what makes the web UI's client-side routing work: any unrecognized path is assumed to be a SPA route, not a missing resource.
- Has a file extension (e.g. `/foo.js`, `/does-not-exist.txt`) -- serves that file if it exists (compiled in, or from `YSR_WEB_DIR` if set), otherwise a real `404 Not Found` with no SPA fallback.

A path with no extension therefore never 404s through this fallback -- a typo'd API route (e.g. `GET /api/entitites`) returns the SPA's HTML instead of a `404` JSON error, which can be surprising when debugging a client. A dotfile-style path (e.g. `/.env`) is also treated as extension-less and falls through to `index.html`, not a straight 404 -- the leading dot is treated as part of the filename, not an extension marker, so `Path::extension()` (and this fallback logic with it) sees no extension at all.

## MCP Tools

Connecting to `/mcp` (Streamable HTTP) gives you access to 22 tools. Example connection from Claude Code:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| Tool | Scope | Description |
|---|---|---|
| `create_schema` | schema | Register a meta-schema (adds a new version), from an inline `definition` or a `template_id` |
| `list_templates` | read | List built-in schema templates usable as `create_schema`'s `template_id` (a template-library UUID works there too) |
| `list_schemas` | read | List a summary of registered schemas (for discovery) |
| `get_active_schema` | read | Fetch the active schema definition |
| `get_schema_by_id` | read | Fetch a specific schema version |
| `get_entity_type_json_schema` | read | Project an entity_type as a JSON Schema |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` | write/read | Entity CRUD |
| `list_entities` | read | List entities, optionally filtered by `entity_type`, a `filter` JSONB containment match, and/or `schema_version` |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | Relation CRUD |
| `set_relation_status` | write | Move a relation to `active`, `deprecated` or `archived`. Traversal follows `active` relations only, so this retires one without destroying the record that it existed |
| `get_entity_drift` | read | Report how an entity stands against the active version of its schema — the fields it predates, and whether the active version requires them |
| `search_entities` | read | Vector similarity search over a natural-language query, optionally narrowed by `entity_type`/`filter`; entities without an embedding can still surface via trigram fuzzy matching |
| `recall_context` | read | Fetch an entity plus its relations and connected neighbors in one call |
| `import_jsonl` | schema | Bulk-import schemas/entities/relations from a JSON Lines document in the export format, as a single transaction |
| `list_template_library` | read | List the tenant's DB-backed schema template library (distinct from `list_templates`, which lists the built-in templates) |
| `get_template_library_item` | read | Fetch a single template from the tenant's DB-backed template library by ID |

The REST-only `GET /api/export.jsonl` endpoint (every schema version in the tenant, plus the workspace's entities and relations, as JSON Lines) has no MCP tool equivalent, but its counterpart `POST /api/import.jsonl` does: `import_jsonl` above.
