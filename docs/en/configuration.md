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

## Database load guard

The PostgreSQL load guard is disabled by default because it changes the deployment-wide maintenance state automatically.

Status values are typed internally while preserving their existing string representation on API and database wire formats.
Set `YORISHIRO_DB_LOAD_THRESHOLD` to the active-connection count that should be treated as busy to enable it.
The guard samples `pg_stat_activity` every 5 seconds by default and changes to read-only only after the threshold remains crossed for 30 seconds.
It returns to normal service after the same quiet period, but only when the guard itself set read-only mode.
An operator-owned read-only or full-lock state is never cleared by the guard.
The guard uses the PostgreSQL identity pool and is a no-op on SQLite.
This is the port of the earlier guard implementation, with its opt-in default retained because automatic maintenance changes require an explicit operational choice.
`cargo loco task db_load_guard` is diagnostic only and reports one sample without changing maintenance mode.

| Variable | Default | Description |
|---|---|---|
| `YORISHIRO_DB_LOAD_THRESHOLD` | Disabled (`0`) | Active PostgreSQL connections at or above which the guard considers the database busy |
| `YORISHIRO_DB_LOAD_SUSTAIN_SECS` | `30` | Seconds the busy or quiet condition must persist |
| `YORISHIRO_DB_LOAD_POLL_SECS` | `5` | Seconds between PostgreSQL activity samples; non-positive values fall back to `5` |

## Main configuration (`yorishiro.yaml`)

`yorishiro.yaml` is the canonical configuration file.
Create a documented skeleton in the current directory with `yorishiro config init`.
The command refuses to replace an existing file unless you pass `--force`.
It logs the replacement before writing when `--force` replaces an existing file.

Yorishiro reads `YORISHIRO_CONFIG_PATH` first when it is set.
That path is strict: a missing, unreadable, or invalid file is an error and does not fall back.
Otherwise Yorishiro reads `./yorishiro.yaml` when present.
Only when neither exists does it use the transitional Loco file at `config/{environment}.yaml`.
Legacy files can still use Loco's Tera syntax during this migration, but do not add that syntax to `yorishiro.yaml`.

The canonical file is plain YAML.
The following environment variables explicitly override its fields, so deploy-time secrets and addresses do not need template interpolation: `DATABASE_URL`, `DB_LOGGING`, `DB_CONNECT_TIMEOUT`, `DB_IDLE_TIMEOUT`, `DB_MIN_CONNECTIONS`, `DB_MAX_CONNECTIONS`, `DB_AUTO_MIGRATE`, `PORT`, `BINDING`, `HOST`, `LOG_LEVEL`, `QUEUE_URL`, `YORISHIRO_QUEUE_KIND`, `YORISHIRO_QUEUE_WORKERS`, `YORISHIRO_QUEUE_REAPER_AGE_MINUTES`, `MAILER_HOST`, `MAILER_PORT`, `MAILER_USER`, and `MAILER_PASSWORD`.

An unconfigured skeleton starts with SQLite locally. No external services are needed.

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc` | `postgres://` URI for multi-tenant or vector search |
| `HOST` | `http://localhost` | Your server's hostname or address |
| `YORISHIRO_QUEUE_KIND` | YAML value | `Sqlite`, `Postgres`, or `Redis` |
| `QUEUE_URL` | YAML value | Queue connection URI |
| `YORISHIRO_EMBEDDING_BASE_URL` | Unset | Embeddings endpoint URL |
| `YORISHIRO_EMBEDDING_MODEL` | Unset | Embeddings model name |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | Embedding provider (`none`, `local`, or unset for local) |

### Mailer

Email is not used by Yorishiro by default.
Add a `mailer:` block when required, then use `MAILER_HOST`, `MAILER_PORT`, `MAILER_USER`, and `MAILER_PASSWORD` for deploy-time overrides.

### Logging

Yorishiro uses the `LOG_LEVEL` environment variable to set logging verbosity (`debug`, `info`, `warn`, `error`). The `RUST_LOG` variable also works and takes priority over everything else.

## Workers and queues (background jobs)

A worker is an executable process that dequeues and runs background jobs.
A worker pool is a target group selected by `WorkerClass`: `TenantPrivate`, `Official`, or `Shared`.
The queue is durable job backlog storage, not a worker pool or a database connection pool.

Run the HTTP server and workers as separate processes.
The supported all-job worker command is:

```
cargo loco start --worker=worker-class:tenant-private,worker-class:official,worker-class:shared,infer-fill
```

Run a pool-specific worker when that process should consume only one embedding and reindex target group:

```
cargo loco start --worker=worker-class:shared
cargo loco start --worker=worker-class:official
cargo loco start --worker=worker-class:tenant-private
```

`infer-fill` is a separate tagged job type, so an infer-fill-only process uses `cargo loco start --worker=infer-fill`.
The all-job command includes it as well as every worker-pool tag.

Do not use a bare `cargo loco start --worker`, `--server-and-worker`, or `--all` for Yorishiro background jobs.
Loco passes an empty tag list for those modes, which consumes only untagged jobs.
Yorishiro's registered jobs are tagged, so those modes do not consume them.

Every worker process needs the same queue configuration and embedding or inference configuration that its jobs require.
Workers may run on the same host as the HTTP server or on a separate server.
Separate-server workers must use the same `QUEUE_URL` as the server.
When that URL names a shared SQLite file, use an absolute path because a relative path is resolved independently on each host and is not a shared queue.
With PostgreSQL, multiple worker processes can dequeue in parallel.
With SQLite, dequeueing is serialized, while durable queue recovery still works.

`YORISHIRO_QUEUE_WORKERS` controls the number of concurrent dequeue loops in one worker process.
`YORISHIRO_QUEUE_REAPER_AGE_MINUTES` controls how long a job may remain processing before the reaper recovers it.

## Tenant reindex schedule

You can configure a deployment to automatically reindex every workspace under a tenant on a regular interval. The schedule runs through the normal reindex flow (same as the manual `reindex_embeddings` task), so it respects the same workspace provider and model version checks.

Configure the schedule through the API endpoint (`POST /api/identity/tenants/schedule` or the equivalent route), which takes an ISO 8601 duration (`P1D` for daily, `P1W` for weekly) and an optional IANA timezone name (default: UTC). The scheduler picks up the next tick within a five-minute grace window, so a missed run is not lost if the process restarts during that window.

To run the scheduler on a fixed cron schedule, add a `scheduler:` entry to `yorishiro.yaml` that names the `TenantReindexScheduler` task. See the Loco documentation for the scheduler configuration format.

## SQLite ANN benchmark

When you run Yorishiro on SQLite with a large number of embedded entities, full-scan vector search may become slow. This deployment ships a benchmark task that measures the actual latency on your data and recommends whether to adopt the vec0 virtual table (KNN index).

```
cargo loco task sqlite_ann_benchmark workspace_id:<uuid>
```

The task measures median, min, and max latency across five iterations of a vector search query. When the entity count is 1000 or more and the median latency exceeds 200 ms, the task recommends adopting vec0. You can adjust both thresholds with `min_entities` and `max_latency_ms` CLI arguments.

This task is measurement only: it does not change any schema or configuration. It helps you decide whether the vec0 virtual table is worth adopting on your deployment.

## Secret handling in logs

API keys, webhook secrets, OAuth credentials, OIDC tokens, and licence owner information are hidden from debug logs.
Provider error responses are also omitted from logs because they may echo credentials or tokens.
This covers deployment and workspace embedding or LLM API keys, Stripe webhook secrets, OAuth client and state-signing credentials, OIDC ID tokens, and the subject in a licence claim.
