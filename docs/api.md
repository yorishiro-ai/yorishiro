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

`GET /api/entities` also accepts a `filter` query parameter (a JSON object matched with JSONB containment, e.g. `filter={"status":"active"}`), and `POST /api/schemas` accepts either an inline definition or `{"template_id": "..."}` to register one of the built-in templates listed at `GET /api/templates`.

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

## Unmatched paths (web UI fallback)

Any request path that isn't one of the API routes above falls through to the web UI's static file server, and its behavior depends on whether the path *looks like* a file:

- No file extension (e.g. `/foo`, `/dashboard`, `/schemas/abc`) -- always serves the SPA's `index.html`, `200 OK`. This is what makes the web UI's client-side routing work: any unrecognized path is assumed to be a SPA route, not a missing resource.
- Has a file extension (e.g. `/foo.js`, `/does-not-exist.txt`) -- serves that file if it exists (compiled in, or from `YSR_WEB_DIR` if set), otherwise a real `404 Not Found` with no SPA fallback.

A path with no extension therefore never 404s through this fallback -- a typo'd API route (e.g. `GET /api/entitites`) returns the SPA's HTML instead of a `404` JSON error, which can be surprising when debugging a client. A dotfile-style path (e.g. `/.env`) is also treated as extension-less and falls through to `index.html`, not a straight 404 -- the leading dot is treated as part of the filename, not an extension marker, so `Path::extension()` (and this fallback logic with it) sees no extension at all.

## MCP Tools

Connecting to `/mcp` (Streamable HTTP) gives you access to 20 tools. Example connection from Claude Code:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| Tool | Scope | Description |
|---|---|---|
| `create_schema` | schema | Register a meta-schema (adds a new version), from an inline `definition` or a `template_id` |
| `list_templates` | read | List built-in schema templates usable as `create_schema`'s `template_id` |
| `list_schemas` | read | List a summary of registered schemas (for discovery) |
| `get_active_schema` | read | Fetch the active schema definition |
| `get_schema_by_id` | read | Fetch a specific schema version |
| `get_entity_type_json_schema` | read | Project an entity_type as a JSON Schema |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` | write/read | Entity CRUD |
| `list_entities` | read | List entities, optionally filtered by `entity_type` and/or a `filter` JSONB containment match |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | Relation CRUD |
| `search_entities` | read | Vector similarity search over a natural-language query, optionally narrowed by `entity_type`/`filter`; entities without an embedding can still surface via trigram fuzzy matching |
| `recall_context` | read | Fetch an entity plus its relations and connected neighbors in one call |
| `import_jsonl` | schema | Bulk-import schemas/entities/relations from a JSON Lines document in the export format, as a single transaction |
| `list_template_library` | read | List the tenant's DB-backed schema template library (distinct from `list_templates`, which lists the built-in templates) |
| `get_template_library_item` | read | Fetch a single template from the tenant's DB-backed template library by ID |

The REST-only `GET /api/export.jsonl` endpoint (every schema version in the tenant, plus the workspace's entities and relations, as JSON Lines) has no MCP tool equivalent, but its counterpart `POST /api/import.jsonl` does: `import_jsonl` above.
