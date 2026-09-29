# Enterprise configuration

[English](../../docs/en/configuration.md) | [日本語](../../docs/ja/configuration.md)

Enterprise edition features that require additional configuration.

## Per-workspace embedding providers

Each workspace can use its own embedding endpoint. Use `PUT /api/workspace/embedding-key` to assign a different provider to a single workspace.

| Field | Description |
|---|---|
| `base_url` | Embeddings endpoint URL |
| `model` | Model name |
| `api_key` | API key (never returned by GET) |
| `dimensions` | Vector size the model produces |
| `send_dimensions_param` | Include dimensions in requests (default: `false`) |

## OAuth2/OIDC login

OAuth2/OIDC login fetches the provider discovery document, exchanges the authorization code, and retrieves the provider JWKS through a private OAuth-specific transport seam.
The production implementation owns the `reqwest` client, the ten-second timeout, HTTPS-or-loopback URL validation, redirect restrictions, response parsing, and credential redaction.
Tests can substitute those three provider operations without changing authorization, callback, PKCE, state-token, cookie, or user-provisioning behavior.
Loco provides application composition and queue extension points, but it does not provide an extension point for this provider-specific outbound transport, so the seam is composed inside the existing OAuth service instead of adding a generic HTTP service.

## Asynchronous infer-fill

Configure a workspace LLM key with `PUT /api/workspace/llm-key`, then start a fill with `POST /api/schemas/active/{name}/infer-fill`.
The response contains a durable `job_id` and the initial `queued` status.
Poll `GET /api/inference-jobs/{job_id}` with a read-scoped key until the status is `completed` or `failed`.
The job record stores its status, proposed, applied, and skipped counts, and a serialized error message, so polling continues to work after a server restart.
Infer-fill stores each model answer as a `pending` proposal instead of writing guessed data to the entity.
List proposals with `GET /api/inference-jobs/{job_id}/proposals`, then explicitly confirm, reject, or discard them with `POST /api/inference-jobs/{job_id}/confirm`, `/reject`, or `/discard`.
Confirmation validates the merged entity against the target schema, snapshots accepted writes for `POST /api/migration-jobs/{job_id}/undo`, and marks stale or invalid proposals without changing the entity.
Proposal delivery is idempotent for the same job, entity, schema version, and source field.
All proposal actions require the workspace's schema scope, while listing requires read scope.
The workspace key is never included in job records, proposal responses, or logs, and a missing workspace key rejects the request before a job is created.
The job is claimed only while it is `queued`.
If a worker crashes after claiming it, the durable row remains `running` and is not automatically retried, because retrying could start a second inference while the original worker is still active.
Operators must reconcile a `running` job before retrying it through an operational procedure.
Job records are retained until a future retention policy is introduced.

The OpenAI-compatible chat-completions call stays behind a private inference-client transport seam.
The production implementation uses the configured `reqwest` client, while tests can substitute the network without changing infer-fill behavior or exposing a generic application-wide HTTP service.
No Loco extension applies because Loco provides application composition and queue extension points, not a provider-specific outbound transport for this call.

## Per-workspace worker class

`PUT /api/workspace/worker-class` assigns a workspace's background jobs to a specific compute pool.

| Field | Description |
|---|---|
| `worker_class` | `tenant_private`, `official`, or `shared` |

## Compute policy foundations

This release defines and tests plan-derived policy primitives, but does not activate them in production queue execution.
Loco Queue does not expose provider-neutral demand observation, and the current compute path has no actual-cost source.
Therefore this release does not enforce queue priority or SLA objectives, route automatic Official bursts, or charge compute automatically.

| Plan | Priority | Queue-start objective | Official base | Burst ceiling |
|---|---|---:|---:|---:|
| Free | low | best effort | 1 | 2 |
| Pro | normal | 60 seconds | 4 | 6 |
| Team | high | 15 seconds | 8 | 12 |

The pure burst predicate requires observed official demand above the base limit, within the ceiling, and positive credits.
The routing hook seam preserves explicit assignments and never implicitly selects TenantPrivate.
`debit_actual` provides an append-only, no-overdraft debit primitive for a future caller that can provide an actual cost.
Provider-neutral demand observation, an explicit actual-cost callback, production routing, and post-success charging remain follow-up work.

## Schema origin notifications

Each linked workspace schema is its own subscriber to template updates.
Updating a template definition sets the linked schema's `origin_updated_at` to `NULL`, and repeating the update is idempotent.
The MCP upstream-change pull is the discovery surface and includes a computed merge summary with total fields, automatic additions and updates, local fields, conflicts, and a conflict flag.
Applying a conflict-free merge creates the new schema version and clears the pending flag in the same transaction.
Conflicted or failed merges leave the notification pending.

## Workspace schema forks

Enterprise workspaces can copy an exact active schema version from another workspace in the same tenant with `POST /api/schema-forks`.
The fork starts detached from template origin tracking and keeps immutable schema history under its own workspace.
`PUT /api/schema-forks/{fork_id}` accepts either a local definition update or the explicit `action=follow` operation.
Following is synchronous and requires `force=true` when local edits would be discarded.
The fork metadata is separate from `template_templates.fork_of`, which describes template-library relationships rather than workspace schema lineage.
