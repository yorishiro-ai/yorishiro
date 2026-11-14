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

### `accept_invite`

Accept a tenant invitation as the authenticated human account (requires read scope and matching account email); service credentials cannot accept, no login credential is issued and the selected workspace does not change

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "invite_token": {
      "type": "string"
    }
  },
  "required": [
    "invite_token"
  ],
  "type": "object"
}
```

### `add_group_member`

Add group member in the current tenant (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "group_id",
    "user_id"
  ],
  "type": "object"
}
```

### `archive_schema`

Archive a schema version in the selected workspace without deleting entities, version history or fork references; no older version is reactivated and future same-name creation advances the version (requires schema scope and schema_archive IAM permission)

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

### `attach_group_policy`

Attach group policy in the current tenant (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
    "policy_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "group_id",
    "policy_id"
  ],
  "type": "object"
}
```

### `attach_user_policy`

Attach a tenant IAM policy to a non-root tenant user (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "policy_id": {
      "format": "uuid",
      "type": "string"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "user_id",
    "policy_id"
  ],
  "type": "object"
}
```

### `capabilities`

Inspect credential scope, independent audit grant, tenant root identity and visible tools' scope requirements; IAM is evaluated per operation and this snapshot does not authorize requests (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `create_api_key`

Issue an API key and return its secret once; workspace-bound by default, tenant-scoped (X-Workspace-Id required for workspace operations) when tenant_scoped is true, which also requires a tenant-scoped issuing credential; cannot exceed issuer scope, audit grant or expiry (requires read scope and credential_create IAM permission with a tenant-scoped credential)

Input schema:

```json
{
  "$defs": {
    "ApiKeyScope": {
      "description": "Permission level held by an API key.\nDeclaration order feeds the derived `Ord`: `Read < Write < Schema < Migration`, a higher scope subsumes lower ones.\nThe wire form matches the DB `scope` column ('read'/'write'/'schema'/'migration').",
      "oneOf": [
        {
          "enum": [
            "read",
            "write",
            "schema"
          ],
          "type": "string"
        },
        {
          "const": "migration",
          "description": "Running a batch migration, and switching maintenance mode.\nAbove `schema` because both act on data already stored.",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "audit": {
      "default": false,
      "type": "boolean"
    },
    "expires_at": {
      "format": "date-time",
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "type": "string"
    },
    "scope": {
      "anyOf": [
        {
          "$ref": "#/$defs/ApiKeyScope"
        },
        {
          "type": "null"
        }
      ]
    },
    "tenant_scoped": {
      "default": false,
      "description": "Issue a tenant-scoped key (no workspace binding, `X-Workspace-Id` for workspace operations) instead of a workspace-bound one.\nRequires tenant Admin or Owner and a tenant-scoped issuing credential.",
      "type": "boolean"
    }
  },
  "required": [
    "name"
  ],
  "type": "object"
}
```

### `create_entity`

Create a new entity (requires write scope and entity_create IAM permission)

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

### `create_iam_group`

Create a tenant IAM group (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
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

### `create_iam_policy`

Create a tenant IAM policy (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "document": true,
    "name": {
      "type": "string"
    }
  },
  "required": [
    "name",
    "document"
  ],
  "type": "object"
}
```

### `create_invite`

Create an expiring tenant invitation and return its token for caller delivery, without sending email (requires read scope and tenant_user_create IAM permission; Owner invitations are rejected)

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

Create a tenant-shared schema template in the library, not a workspace data schema (requires read scope and template_library_create IAM permission)

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

### `create_workspace`

Create a workspace in the current tenant without retargeting the current credential (requires read scope, a tenant-scoped credential and workspace_create IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "max_entities": {
      "format": "int32",
      "type": [
        "integer",
        "null"
      ]
    },
    "name": {
      "type": "string"
    },
    "schema_id": {
      "format": "uuid",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "required": [
    "name"
  ],
  "type": "object"
}
```

### `delete_entity`

Delete an entity (requires write scope and entity_delete IAM permission)

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

### `delete_iam_group`

Delete a tenant IAM group and its memberships and policy attachments (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "group_id"
  ],
  "type": "object"
}
```

### `delete_iam_policy`

Delete a tenant IAM policy and its user and group attachments (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "policy_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "policy_id"
  ],
  "type": "object"
}
```

### `delete_relation`

Delete a relation (requires write scope and relation_delete IAM permission)

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

### `delete_schema`

Delete an archived schema version that nothing references (no entities, snapshots, inference proposals, workspaces or forks); an active version or any remaining reference is refused with the blockers named (requires schema scope and schema_delete IAM permission)

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

### `delete_template_library_item`

Delete an owned tenant template while preserving existing workspace schemas (requires read scope and template_library_delete IAM permission)

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

### `delete_workspace`

Permanently delete a current-tenant workspace and its data; the last workspace is protected (requires read scope and workspace_delete IAM permission within the credential workspace binding)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "workspace_id"
  ],
  "type": "object"
}
```

### `detach_group_policy`

Detach group policy in the current tenant (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
    "policy_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "group_id",
    "policy_id"
  ],
  "type": "object"
}
```

### `detach_user_policy`

Detach a tenant IAM policy from a tenant user (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "policy_id": {
      "format": "uuid",
      "type": "string"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "user_id",
    "policy_id"
  ],
  "type": "object"
}
```

### `disable_tenant`

Disable the current tenant after confirming its UUID, revoking all its credentials and cancelling work; data and identities remain and quota stays occupied (requires read scope and tenant_update IAM permission with a tenant-scoped credential; physical deletion is operator-only)

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

Discover workspace candidates within the current credential's tenant; workspace-bound credentials list only their bound workspace, and discovery does not grant access (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `export_jsonl`

Export schemas, entities, and relations in the selected workspace as newline-delimited JSON (requires read scope); one {"kind":"schema"|"entity"|"relation","record":{...}} object per line, the same format import_jsonl reads back. Limited to 8 MiB and 10,000 records; larger workspaces must use paginated list APIs.

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `fill_defaults`

Enqueue a fill-defaults job (requires migration scope and migration_fill_defaults IAM permission, rechecked at execution). Returns a job ID; the worker fills absent required fields and takes undo snapshots. Completed counts appear in the audit log.

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

Fork a visible schema template into the current tenant's library (requires read scope and template_library_fork IAM permission)

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

Get the currently active schema definition by name (requires read scope and schema_get IAM permission)

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

### `get_current_user`

Read your own user profile, including a tenant user's name and tenant, without exposing password hashes or OAuth subjects; service credentials are denied (requires read scope)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `get_entity`

Get a single entity by ID (requires read scope and entity_get IAM permission)

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

### `get_iam_group`

Get a tenant IAM group (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "group_id"
  ],
  "type": "object"
}
```

### `get_iam_policy`

Get a tenant IAM policy (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "policy_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "policy_id"
  ],
  "type": "object"
}
```

### `get_relation`

Get a single relation by ID (requires read scope and relation_get IAM permission)

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

Get a specific version of a schema definition by ID (requires read scope and schema_get IAM permission)

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

### `get_tenant`

Read the current tenant's basic metadata and stored manual workspace cap (requires read scope and IAM permission)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `get_workspace`

Get workspace details, counts and effective entity cap with workspace_get IAM permission; cannot retarget a selected or credential-bound workspace (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "workspace_id"
  ],
  "type": "object"
}
```

### `grant_workspace_member`

Grant or change a direct workspace role for an existing tenant member; grants no tenant administration or key issuance authority (requires read scope and workspace_update IAM permission)

Input schema:

```json
{
  "$defs": {
    "WorkspaceRole": {
      "description": "Workspace-local authority never grants tenant administration.",
      "enum": [
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
      "$ref": "#/$defs/WorkspaceRole"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    },
    "workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "workspace_id",
    "user_id",
    "role"
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

Bulk-import schemas/entities/relations from a JSON Lines document in the export format (requires schema scope, since importing schemas is itself a schema-scope-only operation). Returns a job_id; poll import_job_status. Requires import_create IAM permission at enqueue and execution. The worker applies every record in one transaction or rolls everything back.

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

### `list_api_keys`

List current-tenant API key history including revoked and expired grants without secrets (requires read scope and credential_list IAM permission with a tenant-scoped credential)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `list_audit_log`

List the selected workspace's paginated audit history (requires audit_log_list IAM permission and the independent audit grant; administration does not grant audit access)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "page": {
      "description": "One-based page number.",
      "format": "uint64",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    },
    "page_size": {
      "format": "uint64",
      "minimum": 0,
      "type": [
        "integer",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `list_entities`

List entities (requires read scope and entity_list IAM permission)

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

### `list_iam_audit`

List tenant IAM audit events (requires read scope, audit grant and IAM permission)

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

### `list_iam_group_members`

List tenant IAM group members with bounded pagination (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
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
  "required": [
    "group_id"
  ],
  "type": "object"
}
```

### `list_iam_group_policies`

List tenant IAM group policies with bounded pagination (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
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
  "required": [
    "group_id"
  ],
  "type": "object"
}
```

### `list_iam_groups`

List tenant IAM groups with bounded pagination (requires read scope and IAM permission)

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

### `list_iam_policies`

List tenant IAM policies with bounded pagination (requires read scope and IAM permission)

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

### `list_iam_user_policies`

List tenant IAM user policies with bounded pagination (requires read scope and IAM permission)

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
    },
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

### `list_invites`

List current-tenant invitations without exposing their tokens (requires read scope and tenant_user_list IAM permission with a tenant-scoped credential)

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

List members of the current tenant (requires read scope and tenant_user_list IAM permission with a tenant-scoped credential; service credentials are rejected)

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

List relations (requires read scope and relation_list IAM permission)

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

### `list_workspace_members`

List direct workspace grants; inherited tenant administrator access is not a direct grant (requires read scope and workspace_get IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "workspace_id"
  ],
  "type": "object"
}
```

### `list_workspaces`

List current-tenant workspaces allowed by workspace_get IAM permission (requires read scope, workspace_list IAM permission and a tenant-scoped credential)

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

Get a migration job's status (requires migration scope and migration_job_get IAM permission)

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

Preview tenant membership removal and credential revocation without changing data (requires read scope and tenant_user_delete IAM permission with a tenant-scoped credential)

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

Preview a tenant member role change and its key/workspace impact without changing data (requires read scope and tenant_user_update IAM permission with a tenant-scoped credential)

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

Fetch an entity's full body together with its relations and connected neighbors, up to `depth` hops away, in one call (requires read scope and recall IAM permission)

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

Enqueue workspace embedding reindex (requires migration scope and workspace_reindex IAM permission, rechecked before execution and publication)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `reissue_invite`

Replace an unused current-tenant invitation token and expiry, invalidating the old token, for caller delivery (requires read scope and tenant_user_update IAM permission with a tenant-scoped credential)

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

### `remove_group_member`

Remove group member in the current tenant (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
    "user_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "group_id",
    "user_id"
  ],
  "type": "object"
}
```

### `remove_member`

Remove a current-tenant membership and revoke attributed tenant credentials, retaining a root user's account while a tenant user's account is deleted with its membership (requires read scope and tenant_user_delete IAM permission with a tenant-scoped credential; last Owner is protected)

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

### `revoke_api_key`

Revoke a current-tenant API key while retaining secret-free history (requires read scope and credential_revoke IAM permission with a tenant-scoped credential)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "key_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "key_id"
  ],
  "type": "object"
}
```

### `revoke_invite`

Cancel an unused current-tenant invitation; repeated cancellation is safe (requires read scope and tenant_user_delete IAM permission with a tenant-scoped credential)

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

### `revoke_workspace_member`

Revoke a direct workspace grant and affected workspace-bound credentials; inherited tenant authority remains (requires read scope and workspace_update IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "user_id": {
      "format": "uuid",
      "type": "string"
    },
    "workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "workspace_id",
    "user_id"
  ],
  "type": "object"
}
```

### `rotate_api_key`

Atomically rotate a current-tenant API key, revoke its predecessor and return the successor secret once (requires read scope and credential_rotate IAM permission with a tenant-scoped credential; issuer ceilings apply)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "key_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "key_id"
  ],
  "type": "object"
}
```

### `search_entities`

Search entities by vector similarity and selective literal text; PostgreSQL uses gated rank fusion, SQLite uses text fallback (requires read scope and search IAM permission)

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
      "description": "Natural-language query text.\nVectorized via the embedding provider and matched against entities' `x-embed` field by cosine distance.\nPostgreSQL also fuses NFKC-normalized literal text matches when at most 20 entities match.\nSQLite fills remaining slots from unembedded entities using quoted FTS5 terms of at least three characters.",
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

Add an existing user to the current tenant or change their tenant role, returning existing key grant metadata; a tenant user can only join its own tenant and never as Owner (requires read scope and tenant_user_update IAM permission with a tenant-scoped credential; Owner changes require Owner)

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

Inspect current-tenant retirement inventory and retention consequences without changing data (requires read scope and tenant_get IAM permission with a tenant-scoped credential)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `transfer_ownership`

Reject tenant ownership transfer because root identity is immutable (requires read scope and tenant_user_update IAM permission)

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

Enqueue undo of a migration job's snapshots (requires migration scope and migration_undo IAM permission, rechecked at execution)

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

### `update_current_user`

Update or clear your own display name (shared across tenants for a root user), not another user's identity; email, password and OAuth subjects cannot be changed here (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "display_name": {
      "description": "New display name.\nNull clears it.\nEmail, password and OAuth subjects are not editable here.",
      "type": [
        "string",
        "null"
      ]
    }
  },
  "type": "object"
}
```

### `update_entity`

Replace the data of an existing entity (requires write scope and entity_update IAM permission); pass expected_updated_at from a prior read to refuse the write with a conflict when the entity changed in between

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "properties": {
    "data": {
      "description": "Replacement entity body.\nValidated against the schema version in effect when the entity was created."
    },
    "expected_updated_at": {
      "description": "The entity's `updated_at` as the client read it; a mismatch is refused with a conflict so a stale writer cannot clobber a newer write.",
      "format": "date-time",
      "type": [
        "string",
        "null"
      ]
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

### `update_iam_group`

Rename a tenant IAM group (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "group_id": {
      "format": "uuid",
      "type": "string"
    },
    "name": {
      "type": "string"
    }
  },
  "required": [
    "group_id",
    "name"
  ],
  "type": "object"
}
```

### `update_iam_policy`

Update a tenant IAM policy and immediately re-evaluate its grants (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "document": true,
    "name": {
      "type": "string"
    },
    "policy_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "policy_id",
    "name",
    "document"
  ],
  "type": "object"
}
```

### `update_template_library_item`

Update an owned tenant template; existing workspace schema versions are unchanged (requires read scope and template_library_update IAM permission)

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

### `update_tenant`

Update current tenant name or explicitly replace/clear its manual workspace cap without altering plan quotas (requires read scope and IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "max_workspaces": {
      "format": "int32",
      "type": [
        "integer",
        "null"
      ]
    },
    "name": {
      "type": [
        "string",
        "null"
      ]
    },
    "replace_max_workspaces": {
      "default": false,
      "description": "Explicitly replace the manual workspace cap, including clearing it with null.",
      "type": "boolean"
    }
  },
  "type": "object"
}
```

### `update_workspace`

Update a workspace name with workspace_update IAM permission; replacing or clearing its manual entity cap also requires tenant_update and a tenant-scoped credential; schema binding is immutable here (requires read scope)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "max_entities": {
      "format": "int32",
      "type": [
        "integer",
        "null"
      ]
    },
    "name": {
      "type": [
        "string",
        "null"
      ]
    },
    "replace_max_entities": {
      "default": false,
      "type": "boolean"
    },
    "workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "workspace_id"
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

### `create_schema_fork`

Create a workspace-owned fork of a same-tenant source schema (requires schema scope and schema_fork_create IAM permission); validates source identity and workspace cycles without modifying the source

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "source_schema_id": {
      "format": "uuid",
      "type": "string"
    },
    "source_workspace_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "source_workspace_id",
    "source_schema_id"
  ],
  "type": "object"
}
```

### `delete_schema_fork`

Delete a selected-workspace fork association using the REST reference-preserving model (requires schema scope and schema_fork_delete IAM permission); does not delete the source schema

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "fork_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "fork_id"
  ],
  "type": "object"
}
```

### `get_embedding_provider`

Read the selected workspace's embedding provider metadata without its API key (requires read scope and embedding_provider_get IAM permission); returns not found when the deployment default is used

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `get_llm_provider`

Read the selected workspace's inference LLM endpoint and model without its API key (requires read scope and llm_provider_get IAM permission); returns not found when unconfigured

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `get_schema_fork`

Read a schema fork owned by the selected workspace, including its immutable current head (requires read scope and schema_fork_get IAM permission)

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "fork_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "fork_id"
  ],
  "type": "object"
}
```

### `get_tenant_overview`

Read current tenant billing plan metadata, effective workspace cap, usage and member summary (requires read scope, tenant_get and tenant_user_list IAM permissions with a tenant-scoped credential); this read never changes billing or plan policy

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `get_worker_class`

Read the selected workspace's explicit worker-class assignment (requires read scope and worker_class_get IAM permission); missing assignments use shared compute and return not found here

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `list_entity_columns`

List the selected workspace's stored entity-table column preferences (requires read scope and entity_columns_get IAM permission); these display preferences are shared by workspace users

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

### `list_schema_forks`

List schema forks owned by the selected workspace (requires read scope and schema_fork_list IAM permission); source visibility does not authorize changing the source workspace

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `list_upstream_changes`

List schemas in this workspace whose origin template has changed since the schema was last copied (requires read scope and schema_upstream_list IAM permission). Use this to discover which schemas have pending upstream updates that have not yet been merged.

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

Merge the origin template into a schema, writing the merged definition as the schema's next version (requires schema scope and schema_merge_apply IAM permission). Returns the new schema version and a diff describing whether the merge was breaking. Fails if there are merge conflicts.

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

Preview what merging the origin template would do to a schema (requires read scope and schema_merge_preview IAM permission). Returns a list of fields that would be added, updated, kept, or conflicted. Does not write anything.

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

### `reset_embedding_provider`

Remove the selected workspace's embedding provider override and queue a full reindex atomically using the deployment default (requires schema scope and embedding_provider_delete IAM permission)

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `reset_entity_columns`

Reset the selected workspace's entity-table columns for one entity type to schema-derived defaults (requires write scope and entity_columns_delete IAM permission); this removes only the display preference

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "entity_type": {
      "type": "string"
    }
  },
  "required": [
    "entity_type"
  ],
  "type": "object"
}
```

### `reset_llm_provider`

Remove the selected workspace's inference LLM credentials (requires schema scope and llm_provider_delete IAM permission); future inference requires configuration again, and already-started external calls are not cancelled

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `reset_worker_class`

Remove the selected workspace's worker-class assignment and fall back to shared compute (requires schema scope and worker_class_delete IAM permission); queued jobs retain their existing routing

Input schema:

```json
{
  "properties": {},
  "type": "object"
}
```

### `set_embedding_provider`

Configure the selected workspace's OpenAI-compatible embedding provider (requires schema scope and embedding_provider_update IAM permission); document changes queue a full reindex atomically, query-instruction-only changes do not; the supplied API key is stored and never returned

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "api_key": {
      "description": "Stored as given and never returned.",
      "type": "string"
    },
    "base_url": {
      "type": "string"
    },
    "dimensions": {
      "format": "int32",
      "type": "integer"
    },
    "document_prefix": {
      "default": "",
      "type": "string"
    },
    "model": {
      "type": "string"
    },
    "query_instruction": {
      "default": "",
      "type": "string"
    },
    "send_dimensions_param": {
      "default": false,
      "type": "boolean"
    }
  },
  "required": [
    "base_url",
    "model",
    "api_key",
    "dimensions"
  ],
  "type": "object"
}
```

### `set_entity_columns`

Replace the selected workspace's visible columns for one entity type (requires write scope and entity_columns_update IAM permission); validates duplicate columns and the display limit without changing schema definitions

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "columns": {
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "entity_type": {
      "type": "string"
    }
  },
  "required": [
    "entity_type",
    "columns"
  ],
  "type": "object"
}
```

### `set_llm_provider`

Configure the selected workspace's inference LLM endpoint, model and API key (requires schema scope and llm_provider_update IAM permission); stores the secret without returning it or invoking inference

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "api_key": {
      "description": "Stored as given and never returned.",
      "type": "string"
    },
    "base_url": {
      "type": "string"
    },
    "model": {
      "type": "string"
    }
  },
  "required": [
    "base_url",
    "model",
    "api_key"
  ],
  "type": "object"
}
```

### `set_worker_class`

Assign the selected workspace's worker class (requires schema scope and worker_class_update IAM permission); the deployment must declare the destination worker target, which does not guarantee a worker is online

Input schema:

```json
{
  "$defs": {
    "WorkerClass": {
      "description": "What kind of compute a queued job is meant for.\n\nThe class is recorded on the job's lifecycle row and decides its priority band and concurrency limit.\nIt does not choose the worker: that is the job's [`crate::workers::target::WorkerTarget`], which a `TenantPrivate` job derives from the tenant that owns its workspace.",
      "oneOf": [
        {
          "const": "tenant_private",
          "description": "Runs only on compute a single tenant registered for its own workspaces.",
          "type": "string"
        },
        {
          "const": "official",
          "description": "Runs only on compute this deployment operates itself.",
          "type": "string"
        },
        {
          "const": "shared",
          "description": "Runs on any worker process willing to take the job; the default for a deployment with no registered compute of its own.",
          "type": "string"
        }
      ]
    }
  },
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "worker_class": {
      "$ref": "#/$defs/WorkerClass"
    }
  },
  "required": [
    "worker_class"
  ],
  "type": "object"
}
```

### `update_schema_fork`

Update a selected-workspace schema fork with a definition or action=follow using the REST model's history and expected-head guards (requires schema scope and schema_fork_update IAM permission); force is required to discard local edits

Input schema:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "additionalProperties": false,
  "properties": {
    "action": {
      "type": [
        "string",
        "null"
      ]
    },
    "definition": true,
    "expected_fork_schema_id": {
      "format": "uuid",
      "type": [
        "string",
        "null"
      ]
    },
    "expected_source_schema_id": {
      "format": "uuid",
      "type": [
        "string",
        "null"
      ]
    },
    "force": {
      "default": false,
      "type": "boolean"
    },
    "fork_id": {
      "format": "uuid",
      "type": "string"
    }
  },
  "required": [
    "fork_id"
  ],
  "type": "object"
}
```

<!-- END GENERATED MCP INVENTORY -->
