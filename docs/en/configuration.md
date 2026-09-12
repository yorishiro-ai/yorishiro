# Configuration

**English** | [日本語](../ja/configuration.md)

Yorishiro is configured through environment variables. No config file is needed for the settings below.

## Embedding (automatic text search)

Yorishiro can turn your text into numbers that enable similarity search. This is called an "embedding."

### How it works

When you create or update an entity, Yorishiro automatically generates an embedding in the background. This happens after the save is complete, so it does not slow down your write.

There is one exception: if you import a backup file (`import_jsonl`), the imported entities do not have embeddings yet. They are still searchable through fuzzy text matching (the words you type are matched against the text), but similarity search results will be incomplete until embeddings are generated. You can generate embeddings for all entities with:

```
cargo loco task resync_embeddings workspace_id:<uuid>
```

An entity without an embedding does not cause errors. Search simply returns less relevant results for it.

### Choosing an embedding provider

You can choose between a local model that runs on your machine, or an OpenAI-compatible service (LM Studio, Ollama, vLLM, OpenAI, etc.).

| Variable | Description |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `none` disables embeddings entirely; `local` runs a model on your machine. If you set both `YORISHIRO_EMBEDDING_BASE_URL` and `YORISHIRO_EMBEDDING_MODEL`, those take priority and use the OpenAI-compatible provider instead |
| `YORISHIRO_EMBEDDING_BASE_URL` | URL of an OpenAI-compatible embeddings endpoint, e.g. `http://localhost:11434`. Requires `YORISHIRO_EMBEDDING_MODEL` |
| `YORISHIRO_EMBEDDING_MODEL` | Model name for the OpenAI-compatible provider |
| `YORISHIRO_EMBEDDING_API_KEY` | API key for the OpenAI-compatible provider. Leave empty for local services |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | Expected vector size (default: `768`). Must match your chosen model |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | Include a `dimensions` field in the request. Default `false` |

### Local model

Set `YORISHIRO_EMBEDDING_PROVIDER=local` to use a model that runs on your machine.

The only available model is `multilingual-e5-base` (768 dimensions), which works well for multiple languages including Japanese.

| Variable | Description |
|---|---|
| `YORISHIRO_LOCAL_MODEL` | Model name (default: `multilingual-e5-base`). An unrecognized value will fail startup |
| `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` | Maximum input length (default: `512`) |

**Downloading the model.** The model file is about 1 GiB and downloads on first use. Subsequent starts use the cached copy. You can also place the files manually at `models/<short_id>/` in the working directory — both the model file and tokenizer file must be present.

If the download fails, restarting fixes it. The binary verifies the downloaded file against a built-in checksum.

### Changing embedding models

If you switch to a different embedding model, existing embedded entities will still use vectors from the old model. You need to regenerate embeddings for affected workspaces:

```
cargo loco task reindex_embeddings workspace_id:<uuid>
```

This re-embeds every entity in the workspace with the new model. Until it finishes, new writes to that workspace will be rejected until the reindex completes.

You can also queue a reindex via the API: `POST /api/migration-jobs/reindex` (requires Migration scope). A reindex also runs automatically on startup if any workspace's model has changed.

## Search token quota

| Variable | Description |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | Tokens per minute a workspace can spend on search (default: `100000`). Charged per query, shared between API and MCP. The default is very high and only matters for runaway automated queries. Japanese text is charged at roughly half the token cost of English with the default estimate, so the quota allows more Japanese queries per minute |

## Production settings (`config/production.yaml`)

An unconfigured `production.yaml` starts with SQLite locally. No external services needed.

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yotsunagi/yorishiro.sqlite3?mode=rwc` | `postgres://` URI for multi-tenant or vector search |
| `HOST` | `http://localhost` | Your server's hostname or address |
| `YORISHIRO_QUEUE_KIND` | Auto-detected from `DATABASE_URL` | Set to `Redis` only if you need a separate queue backend |
| `QUEUE_URL` | Shares `DATABASE_URL` | Required only for Redis |
| `YORISHIRO_EMBEDDING_BASE_URL` | Unset | Embeddings endpoint URL |
| `YORISHIRO_EMBEDDING_MODEL` | Unset | Embeddings model name |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | Embedding provider (`none`, `local`, or unset for local) |

### Mailer

Email is not used by Yorishiro. The mailer block is optional and only renders when `MAILER_HOST` is set.

### Logging

Yorishiro uses the `LOG_LEVEL` environment variable to set logging verbosity (`debug`, `info`, `warn`, `error`). The `RUST_LOG` variable also works and takes priority over everything else.

## Workers (background jobs)

Yorishiro processes embedding generation and other tasks as background jobs. By default, the same process that serves requests also handles jobs.

To run workers in a separate process:

```
cargo loco start --worker
```

This process still needs the same configuration (database URL, embedding settings, etc.) as the main server.

By default, 2 workers run in parallel. You can control this with `YORISHIRO_QUEUE_WORKERS`. The reaper (which recovers jobs from crashed workers) runs every 30 minutes by default and can be tuned with `YORISHIRO_QUEUE_REAPER_AGE_MINUTES`.

Workers can run on a separate machine pointing at the same database and queue URL. With PostgreSQL, multiple worker processes truly run in parallel. With SQLite, they run one at a time (but still provide crash recovery).

## Tenant reindex schedule

You can configure a deployment to automatically reindex every workspace under a tenant on a regular interval. The schedule runs through the normal reindex flow (same as the manual `reindex_embeddings` task), so it respects the same workspace provider and model version checks.

Configure the schedule through the API endpoint (`POST /api/identity/tenants/schedule` or the equivalent route), which takes an ISO 8601 duration (`P1D` for daily, `P1W` for weekly) and an optional IANA timezone name (default: UTC). The scheduler picks up the next tick within a five-minute grace window, so a missed run is not lost if the process restarts during that window.

To run the scheduler on a fixed cron schedule, add a `scheduler:` entry to your `config/*.yaml` that names the `TenantReindexScheduler` task. See the Loco documentation for the scheduler configuration format.

## SQLite ANN benchmark

When you run Yorishiro on SQLite with a large number of embedded entities, full-scan vector search may become slow. This deployment ships a benchmark task that measures the actual latency on your data and recommends whether to adopt the vec0 virtual table (KNN index).

```
cargo loco task sqlite_ann_benchmark workspace_id:<uuid>
```

The task measures median, min, and max latency across five iterations of a vector search query. When the entity count is 1000 or more and the median latency exceeds 200 ms, the task recommends adopting vec0. You can adjust both thresholds with `min_entities` and `max_latency_ms` CLI arguments.

This task is measurement only: it does not change any schema or configuration. It helps you decide whether the vec0 virtual table is worth adopting on your deployment.

## API endpoints

The API reference is served automatically by Swagger UI at `/api/docs`. All REST endpoints, including async infer-fill and its status polling endpoint, are documented there with request and response shapes.

- `POST /api/schemas/active/{name}/infer-fill` — enqueues an async infer-fill job for a schema.
- `GET /api/inference-jobs/{job_id}` — polls the status of an infer-fill job.
- `POST /api/migration-jobs/{job_id}/undo` — reverses the last infer-fill or migration write for a workspace.
- `POST /api/migration-jobs/reindex` — queues a workspace embedding reindex.
