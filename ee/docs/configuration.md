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
