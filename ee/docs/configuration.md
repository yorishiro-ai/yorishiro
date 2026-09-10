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

## Per-workspace worker class

`PUT /api/workspace/worker-class` assigns a workspace's background jobs to a specific compute pool.

| Field | Description |
|---|---|
| `worker_class` | `tenant_private`, `official`, or `shared` |
