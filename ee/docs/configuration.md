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

## Asynchronous infer-fill

Configure a workspace LLM key with `PUT /api/workspace/llm-key`, then start a fill with `POST /api/schemas/active/{name}/infer-fill`.
The response contains a durable `job_id` and the initial `queued` status.
Poll `GET /api/inference-jobs/{job_id}` with a read-scoped key until the status is `completed` or `failed`.
The job record stores its status, applied and skipped counts, and a serialized error message, so polling continues to work after a server restart.
The job is claimed only while it is `queued`.
If a worker crashes after claiming it, the durable row remains `running` and is not automatically retried, because retrying could start a second inference while the original worker is still active.
Operators must reconcile a `running` job before retrying it through an operational procedure.
Job records are retained until a future retention policy is introduced.

## Per-workspace worker class

`PUT /api/workspace/worker-class` assigns a workspace's background jobs to a specific compute pool.

| Field | Description |
|---|---|
| `worker_class` | `tenant_private`, `official`, or `shared` |

## Compute policy

Priority and queue-start objectives are derived from the tenant plan and cannot be set per job.

| Plan | Priority | Queue-start objective | Official base | Burst ceiling |
|---|---|---:|---:|---:|
| Free | low | best effort | 1 | 2 |
| Pro | normal | 60 seconds | 4 | 6 |
| Team | high | 15 seconds | 8 | 12 |

Official burst is eligible only when observed official demand exceeds the base limit, stays within the ceiling, and the workspace has positive compute credits.
An explicit workspace worker-class assignment always wins.
Without an assignment, the WASM offload hook may recommend official compute within these bounds, and all other cases fall back to shared compute.
Tenant-private compute is never selected implicitly.
Actual compute usage is debited from the append-only ledger only after successful work.
Failed or cancelled work is not charged, and insufficient credits never produce an overdraft.

## Schema origin notifications

Each linked workspace schema is its own subscriber to template updates.
Updating a template definition sets the linked schema's `origin_updated_at` to `NULL`, and repeating the update is idempotent.
The MCP upstream-change pull is the discovery surface and includes a computed merge summary with total fields, automatic additions and updates, local fields, conflicts, and a conflict flag.
Applying a conflict-free merge creates the new schema version and clears the pending flag in the same transaction.
Conflicted or failed merges leave the notification pending.
