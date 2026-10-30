# MCP Tools

The inventory below is a checked-in review contract derived from the composed `rmcp` runtime routers.
Runtime discovery and dispatch remain authoritative, so this document must not be used as a runtime registry.
The contract test fails when names, descriptions, or input schemas drift from the generated runtime inventory.

Renaming or removing a tool requires compatibility review before merging.
Prefer a migration or deprecation period when existing clients can reasonably depend on the old name or schema.
An intentional breaking change must update this document and explain the migration decision in the change review.

The inventory does not claim protocol scope metadata.
Authorization remains enforced by the existing centralized MCP authorization path and the tool descriptions only describe required scopes for humans.

<!-- BEGIN GENERATED MCP INVENTORY -->
## Community tools

### `capabilities`

Inspect current grants and visible tools' required scopes and scope satisfaction; this snapshot does not guarantee operation success or authorize later requests (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `create_entity`

Create a new entity (requires write scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "data": {
      "description": "Entity body (JSON object) conforming to the schema's `fields` definition."
    },
    "entity_type": {
      "description": "entity_type name declared in the schema.",
      "type": "string"
    },
    "schema_name": {
      "description": "Name of the schema this entity conforms to.\nThe workspace's current active version is used.",
      "type": "string"
    }
  },
  "required": [
    "schema_name",
    "entity_type",
    "data"
  ],
  "type": "object"
}
```

### `create_invite`

Create an expiring tenant invitation and return its token for caller delivery, without sending email (requires read scope and tenant Admin or Owner; inviting an Owner requires Owner)

Input schema:

```json
{
  "$defs": {
    "MembershipRole": {
      "description": "Mirrors the `tenant_memberships.role` check constraint.",
      "enum": [
        "owner",
        "admin",
        "member",
        "viewer"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "email": {
      "type": "string"
    },
    "expires_at": {
      "format": "date-time",
      "type": "string"
    },
    "role": {
      "$ref": "#/$defs/MembershipRole"
    }
  },
  "required": [
    "email",
    "role",
    "expires_at"
  ],
  "type": "object"
}
```

### `create_relation`

Create a relation between two entities (requires write scope). Properties cannot be edited in place; to change them, delete the relation and recreate it. To retire a relation without losing the record that it existed, use set_relation_status instead of deleting.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "properties": {
      "description": "Arbitrary properties attached to the relation (JSON object, defaults to an empty object if omitted)."
    },
    "relation_type": {
      "description": "relation_type name declared in the schema's `relation_types` definition.",
      "type": "string"
    },
    "source_id": {
      "format": "uuid",
      "type": "string"
    },
    "target_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "source_id",
    "target_id",
    "relation_type"
  ],
  "type": "object"
}
```

### `create_schema`

Register a new schema, or add a new version to an existing schema (requires schema scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "definition": {
      "description": "JSON object conforming to `MetaSchemaDefinition` (name/description/entity_types/relation_types).\nIf a schema with the same name already exists, whether the change is breaking or non-breaking is detected automatically and it is registered as a new version.\nMutually exclusive with `template_id`; exactly one of the two must be set."
    },
    "template_id": {
      "description": "ID of a template to use as the definition instead of supplying one inline.\nA UUID names one from the tenant's own library (see `list_template_library`); anything else names a built-in (see `list_templates`).\nMutually exclusive with `definition`; exactly one of the two must be set.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `create_template_library_item`

Create a tenant-shared schema template in the library, not a workspace data schema (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "author": {
      "type": [
        "string",
        "null"
      ]
    },
    "definition": true,
    "description": {
      "type": [
        "string",
        "null"
      ]
    },
    "locale": {
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "type": "string"
    },
    "tags": {
      "default": [],
      "items": {
        "type": "string"
      },
      "type": "array"
    }
  },
  "required": [
    "name",
    "definition"
  ],
  "type": "object"
}
```

### `delete_entity`

Delete an entity (requires write scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `delete_relation`

Delete a relation (requires write scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `delete_template_library_item`

Delete an owned tenant template while preserving existing workspace schemas (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `disable_tenant`

Disable the current tenant after confirming its UUID, revoking all its credentials and cancelling work; data and identities remain and quota stays occupied (requires read scope and tenant Owner; physical deletion is operator-only)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "confirmed_tenant_id": {
      "description": "Must equal the current tenant UUID, not its display name.",
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "confirmed_tenant_id"
  ],
  "type": "object"
}
```

### `discover_workspaces`

Discover workspace candidates within the current credential's tenant; service credentials list only the selected workspace, and discovery does not grant access (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `fill_defaults`

Enqueue a fill-defaults job (requires migration scope). Returns a job ID; the worker fills absent required fields and takes undo snapshots. Completed counts appear in the audit log.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "schema_name": {
      "description": "Name of the schema whose active version supplies the missing fields.",
      "type": "string"
    }
  },
  "required": [
    "schema_name"
  ],
  "type": "object"
}
```

### `fork_template_library_item`

Fork a visible schema template into the current tenant's library (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    },
    "name": {
      "type": "string"
    }
  },
  "required": [
    "id",
    "name"
  ],
  "type": "object"
}
```

### `get_active_schema`

Get the currently active schema definition by name (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "name": {
      "type": "string"
    }
  },
  "required": [
    "name"
  ],
  "type": "object"
}
```

### `get_entity`

Get a single entity by ID (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `get_entity_drift`

Report how an entity stands against the active version of its schema (requires read scope). Entities are migrated lazily, so one written against an older version can still contain newer fields. Reports active fields absent from stored data, including nested fields in existing objects. Present nulls and type errors are not missing-value diagnostics.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `get_entity_type_json_schema`

Get a specific entity_type within the active schema as a JSON Schema (requires read scope). Use this to let an agent learn field types, required fields, enums, etc. ahead of time.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "entity_type": {
      "description": "entity_type name within that schema.",
      "type": "string"
    },
    "schema_name": {
      "description": "Name of the active schema.",
      "type": "string"
    }
  },
  "required": [
    "schema_name",
    "entity_type"
  ],
  "type": "object"
}
```

### `get_relation`

Get a single relation by ID (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `get_schema_by_id`

Get a specific version of a schema definition by ID (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "schema_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "schema_id"
  ],
  "type": "object"
}
```

### `get_template`

Get a built-in schema template definition by ID (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "template_id": {
      "type": "string"
    }
  },
  "required": [
    "template_id"
  ],
  "type": "object"
}
```

### `get_template_library_item`

Get a single template from the tenant's DB-backed template library by ID (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `import_job_status`

Poll a queued JSON Lines import (requires schema scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "job_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "job_id"
  ],
  "type": "object"
}
```

### `import_jsonl`

Bulk-import schemas/entities/relations from a JSON Lines document in the export format (requires schema scope, since importing schemas is itself a schema-scope-only operation). Returns a job_id; poll import_job_status. The worker applies every record in one transaction or rolls everything back.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "jsonl": {
      "description": "JSON Lines document in the same format `export_jsonl`/`GET /api/export.jsonl` produces: one `{\"kind\":\"schema\"|\"entity\"|\"relation\",\"record\":{...}}` object per line, newline-separated.",
      "type": "string"
    }
  },
  "required": [
    "jsonl"
  ],
  "type": "object"
}
```

### `list_entities`

List entities (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "entity_type": {
      "type": [
        "string",
        "null"
      ]
    },
    "filter": {
      "description": "JSONB containment filter matched against entity data, e.g. `{\"status\": \"active\"}`."
    },
    "limit": {
      "description": "Maximum number of results (defaults to 50 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "description": "Number of records to skip (defaults to 0 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "schema_version": {
      "description": "Restricts results to entities created against this schema version.",
      "format": "int32",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_invites`

List current-tenant invitations without exposing their tokens (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "limit": {
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_members`

List members of the current tenant (requires read scope and tenant Admin or Owner; service credentials are rejected)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "limit": {
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_relations`

List relations (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "limit": {
      "description": "Maximum number of results (defaults to 50 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "description": "Number of records to skip (defaults to 0 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "relation_type": {
      "type": [
        "string",
        "null"
      ]
    },
    "source_id": {
      "format": "uuid",
      "type": [
        "string",
        "null"
      ]
    },
    "status": {
      "description": "Restricts the listing to one state (\"active\", \"deprecated\" or \"archived\").\nOmitted, every state is listed.",
      "type": [
        "string",
        "null"
      ]
    },
    "target_id": {
      "format": "uuid",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_schemas`

List summaries of all schemas registered for the workspace (all versions, including archived). Use this to discover what schemas exist (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "limit": {
      "description": "Maximum number of results (defaults to 50 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "description": "Number of records to skip (defaults to 0 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_template_library`

List the tenant's DB-backed schema template library (own templates plus any community-visible ones). Distinct from `list_templates`, which lists the built-in templates shipped with the server (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "limit": {
      "description": "Maximum number of results (defaults to 50 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "description": "Number of records to skip (defaults to 0 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_templates`

List built-in schema templates that can be used as a starting point for create_schema instead of writing a definition from scratch (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `migration_dry_run`

Count what migrating a schema's entities to its active version would face, without doing it (requires read scope). Reports how many are current, how many are behind without missing required fields, and how many lack a field the active version requires: the last being the work a migration would have to fill in. Inspects stored data, not only schema definitions; this is not full active-schema validation.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "name": {
      "type": "string"
    }
  },
  "required": [
    "name"
  ],
  "type": "object"
}
```

### `migration_job_status`

Get a migration job's status (requires migration scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "job_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "job_id"
  ],
  "type": "object"
}
```

### `preview_member_removal`

Preview tenant membership removal and credential revocation without changing data (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "user_id"
  ],
  "type": "object"
}
```

### `preview_member_role_change`

Preview a tenant member role change and its key/workspace impact without changing data (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$defs": {
    "MembershipRole": {
      "description": "Mirrors the `tenant_memberships.role` check constraint.",
      "enum": [
        "owner",
        "admin",
        "member",
        "viewer"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "role": {
      "$ref": "#/$defs/MembershipRole"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "user_id",
    "role"
  ],
  "type": "object"
}
```

### `recall_context`

Fetch an entity's full body together with its relations and connected neighbors, up to `depth` hops away, in one call (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "depth": {
      "description": "How many hops to traverse outward from the entity (defaults to 1, max 3).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "entity_id": {
      "format": "uuid",
      "type": "string"
    },
    "full": {
      "description": "When true, neighbor entities include every field instead of only `x-embed` fields (defaults to false).",
      "type": [
        "boolean",
        "null"
      ]
    },
    "limit": {
      "description": "Maximum number of relations/neighbors to include per hop (defaults to 20 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "required": [
    "entity_id"
  ],
  "type": "object"
}
```

### `reindex_workspace`

Enqueue workspace embedding reindex (requires migration scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `reissue_invite`

Replace an unused current-tenant invitation token and expiry, invalidating the old token, for caller delivery (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "expires_at": {
      "format": "date-time",
      "type": "string"
    },
    "invite_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "invite_id",
    "expires_at"
  ],
  "type": "object"
}
```

### `remove_member`

Remove a current-tenant membership and revoke attributed tenant credentials, retaining the global user account (requires read scope and tenant Admin or Owner; last Owner is protected)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "user_id"
  ],
  "type": "object"
}
```

### `revoke_invite`

Cancel an unused current-tenant invitation; repeated cancellation is safe (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "invite_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "invite_id"
  ],
  "type": "object"
}
```

### `search_entities`

Vector similarity search over entities using a natural-language query (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "entity_type": {
      "type": [
        "string",
        "null"
      ]
    },
    "filter": {
      "description": "JSONB containment filter matched against entity data, e.g. `{\"status\": \"active\"}`."
    },
    "limit": {
      "description": "Upper bound on the number of results returned (defaults to 10 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "query_text": {
      "description": "Natural-language query text.\nVectorized via the embedding provider and matched against entities' `x-embed` field by cosine distance.\nAlso used, as-is, for an auxiliary pg_trgm fuzzy text match against entities that have no embedding.",
      "type": "string"
    }
  },
  "required": [
    "query_text"
  ],
  "type": "object"
}
```

### `set_member_role`

Add an existing user to the current tenant or change their tenant role, returning existing key grant metadata (requires read scope and tenant Admin or Owner; Owner changes require Owner)

Input schema:

```json
{
  "$defs": {
    "MembershipRole": {
      "description": "Mirrors the `tenant_memberships.role` check constraint.",
      "enum": [
        "owner",
        "admin",
        "member",
        "viewer"
      ],
      "type": "string"
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "role": {
      "$ref": "#/$defs/MembershipRole"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "user_id",
    "role"
  ],
  "type": "object"
}
```

### `set_relation_status`

Set a relation's status to active, deprecated or archived (requires write scope). Retiring a relation this way keeps the record that it existed, which delete_relation does not; graph traversal follows active relations only.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "id": {
      "format": "uuid",
      "type": "string"
    },
    "status": {
      "description": "\"active\", \"deprecated\" or \"archived\".\nTraversal follows \"active\" relations only.",
      "type": "string"
    }
  },
  "required": [
    "id",
    "status"
  ],
  "type": "object"
}
```

### `tenant_retirement_inventory`

Inspect current-tenant retirement inventory and retention consequences without changing data (requires read scope and tenant Owner)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `transfer_ownership`

Atomically transfer tenant ownership to an existing member and demote the caller to Admin (requires read scope and tenant Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "successor_user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "successor_user_id"
  ],
  "type": "object"
}
```

### `undo_migration_job`

Enqueue undo of a migration job's snapshots (requires migration scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "job_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "job_id"
  ],
  "type": "object"
}
```

### `update_entity`

Replace the data of an existing entity (requires write scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "data": {
      "description": "Replacement entity body.\nValidated against the schema version in effect when the entity was created."
    },
    "id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "id",
    "data"
  ],
  "type": "object"
}
```

### `update_template_library_item`

Update an owned tenant template; existing workspace schema versions are unchanged (requires read scope and tenant Admin or Owner)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "definition": true,
    "description": {
      "type": [
        "string",
        "null"
      ]
    },
    "id": {
      "format": "uuid",
      "type": "string"
    },
    "locale": {
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "type": [
        "string",
        "null"
      ]
    },
    "tags": {
      "items": {
        "type": "string"
      },
      "type": [
        "array",
        "null"
      ]
    }
  },
  "required": [
    "id"
  ],
  "type": "object"
}
```

### `whoami`

Inspect the current identity, tenant, selected workspace, effective scope and independent audit grant without exposing credentials (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

## Enterprise-only tools

### `list_upstream_changes`

List schemas in this workspace whose origin template has changed since the schema was last copied (requires read scope). Use this to discover which schemas have pending upstream updates that have not yet been merged.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "limit": {
      "description": "Maximum number of results (defaults to 50 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    },
    "offset": {
      "description": "Number of records to skip (defaults to 0 if omitted).",
      "format": "int64",
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `merge_apply`

Merge the origin template into a schema, writing the merged definition as the schema's next version (requires schema scope). Returns the new schema version and a diff describing whether the merge was breaking. Fails if there are merge conflicts.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "schema_id": {
      "description": "ID of the schema to merge.",
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "schema_id"
  ],
  "type": "object"
}
```

### `merge_preview`

Preview what merging the origin template would do to a schema (requires read scope). Returns a list of fields that would be added, updated, kept, or conflicted. Does not write anything.

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "schema_id": {
      "description": "ID of the schema to preview a merge for.",
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "schema_id"
  ],
  "type": "object"
}
```

<!-- END GENERATED MCP INVENTORY -->
