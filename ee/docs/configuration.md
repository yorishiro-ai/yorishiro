# Enterprise configuration

[English](configuration.md) | [日本語](ja/configuration.md)

Enterprise edition features that require additional configuration.

## Plan policy

`YORISHIRO_PLAN_POLICY_ENABLED` defaults to `false` and accepts only `true` or `false` (case-insensitive).
When enabled, subscription plans enforce workspace and entity limits and target admission limits for background jobs.
Shared jobs default to a deployment-wide limit of `1`; tenant-private jobs default to `1` per tenant; Official jobs share a deployment-wide count with a plan-derived limit (Free `1`, Pro `4`, Team `8`).
Set `YORISHIRO_WORKER_CLASS_SHARED_LIMIT`, `YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT`, or `YORISHIRO_WORKER_OFFICIAL_LIMIT` to override the corresponding target limit with a positive integer.
When disabled, no target admission limits apply, even if the override variables are set.
Enterprise still validates configured overrides at startup.
Plan data remains available for billing and job records, and existing tenant and workspace limits remain unchanged.

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

Set `YORISHIRO_OAUTH_ISSUER_URL`, `YORISHIRO_OAUTH_CLIENT_ID`, and `YORISHIRO_OAUTH_CLIENT_SECRET` to enable login.
Set `YORISHIRO_OAUTH_REDIRECT_URI` to the public callback URL, particularly behind a reverse proxy.
If it is unset, the callback uses the listening host and port, replacing a wildcard host with `localhost`; `YORISHIRO_BIND` overrides this fallback without changing the listener.
The identity provider must use HTTPS unless it runs on a loopback address.

## Asynchronous infer-fill

Configure a workspace LLM key with `PUT /api/workspace/llm-key`, then start a fill with `POST /api/schemas/active/{name}/infer-fill`.
The response contains a durable `job_id` and the initial `queued` status.
Poll `GET /api/inference-jobs/{job_id}` with a read-scoped key until the status is `completed` or `failed`.
The job record stores its status, proposed, applied, and skipped counts, and a serialized error message, so polling continues to work after a server restart.
Infer-fill stores each model answer as a `pending` proposal instead of writing guessed data to the entity.
List proposals with `GET /api/inference-jobs/{job_id}/proposals`, then explicitly confirm, reject, or discard them with `POST /api/inference-jobs/{job_id}/confirm`, `/reject`, or `/discard`.
Confirmation returns 202 with a new confirmation `job_id`; poll `GET /api/inference-jobs/confirmations/{job_id}` with schema scope for its status and result counts.
The confirmation worker validates the merged entity against the target schema, snapshots accepted writes using the original inference job ID for `POST /api/migration-jobs/{job_id}/undo`, and marks stale or invalid proposals without changing the entity.
Rejection and discard remain synchronous.
Proposal delivery is idempotent for the same job, entity, schema version, and source field.
All proposal actions require the workspace's schema scope, while listing requires read scope.
The workspace key is never included in job records, proposal responses, or logs, and a missing workspace key rejects the request before a job is created.
Proposal confirmation, rejection, and discard are allowed only after the parent job is `completed`.
If a job enters `failed`, its pending proposals are automatically marked `discarded` and cannot be confirmed.
Completed proposals remain reviewable until an explicit confirm, reject, or discard action; there is currently no automatic retention cleanup.
Inference jobs do not currently support expiry or cancellation, so those states are not accepted by the API or stored in the database.
The job is claimed only while it is `queued`.
If a worker crashes after claiming it, the durable row remains `running` and is not automatically retried, because retrying could start a second inference while the original worker is still active.
Operators must reconcile a `running` job before retrying it through an operational procedure.
Job records are retained until a future retention policy is introduced.

## Per-workspace worker class

`PUT /api/workspace/worker-class` assigns a workspace's background jobs to a specific compute pool.

| Field | Description |
|---|---|
| `worker_class` | `tenant_private`, `official`, or `shared` |

## Queue priorities

Tenant-private jobs have priority 300, Official jobs 200, and Shared jobs 100.
The plan policy defines queue-start objectives of best effort for Free, 60 seconds for Pro, and 15 seconds for Team, but does not enforce them.
Automatic Official bursts and compute charging are not available.

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
