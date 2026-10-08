# Configuration

**English** | [日本語](../ja/configuration.md)

Yorishiro is configured through environment variables. No config file is needed for the settings below.

## Embedding (automatic text search)

Yorishiro can turn your text into numbers that enable similarity search. This is called an "embedding."

### How it works

When you create or update an entity, Yorishiro automatically generates an embedding in the background. This happens after the save is complete, so it does not slow down your write.

Embeddings are generated in two distinct paths:

**Document embedding (background).** The embedding sync worker (`workers::embedding_sync`) runs as a background job and stores vectors after entity writes. This is the only path that writes to `entity_embeddings`.

**Query embedding (worker-backed).** The search endpoint persists a short-lived query-embedding request and polls it within the configured bound. A query-embedding worker invokes the provider and stores the result; the server does not load a provider or hold a database connection while waiting.

Within a workspace, document and query embeddings resolve the same configured provider and model. Different workspaces may intentionally use different providers, models, and supported widths.

If the worker times out or reports a provider failure, search returns `503`; it does not silently fall back to lexical-only results.

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

The OpenAI-compatible provider keeps the public `EmbeddingProvider` contract as its domain boundary.
Its outbound request is behind a private provider-specific transport seam so failure tests can substitute the network without changing embedding behavior or exposing another application-wide HTTP service.
Loco provides extension points for application composition, but it does not own this provider-specific outbound request, so no Loco transport extension is applicable here.

### Local model

Set `YORISHIRO_EMBEDDING_PROVIDER=local` to use a model that runs on your machine.

The only available model is `multilingual-e5-base` (768 dimensions), which works well for multiple languages including Japanese.

| Variable | Description |
|---|---|
| `YORISHIRO_LOCAL_MODEL` | Model name (default: `multilingual-e5-base`). An unrecognized value will fail startup |
| `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` | Maximum input length (default: `512`) |

**Downloading the model.** The model file is about 1 GiB and downloads on first use. Subsequent starts use the cached copy. You can also place the files manually at `models/<short_id>/` in the working directory — both the model file and tokenizer file must be present.

If the download fails, restarting fixes it. The binary verifies the downloaded file against a built-in checksum.

The download uses a private artifact transport seam, while the embedding artifact component keeps the pinned URL and revision, timeout, expected length, SHA-256 check, partial-file cleanup, and atomic rename together.
Loco's storage extension points do not fit this cache because it is a deployment-local filesystem cache, not application storage, and the verified bytes must be staged beside the destination before a same-filesystem atomic rename.

### Changing embedding models

The deployment supports the closed dimension set `768`, `1024`, and `1536`.
Migrations create all three width partitions up front, even when no current workspace uses some of them.
This keeps workspace-specific provider selection, credential rotation, and reindexing free of runtime table creation and allows different workspaces to coexist at different widths.
Unused partitions contain only small catalog/index metadata; vector storage is charged only to populated partitions.
Workspace and provider configuration selects one supported width, while `reindex_embeddings` moves existing vectors to the new model or width.

If you switch to a different embedding model, existing embedded entities will still use vectors from the old model. You need to regenerate embeddings for affected workspaces:

```
cargo loco task reindex_embeddings workspace_id:<uuid>
```

This re-embeds every entity in the workspace with the new model. Until it finishes, new writes to that workspace will be rejected until the reindex completes.

You can also queue a reindex via the API: `POST /api/migration-jobs/reindex` (requires Migration scope). A reindex also runs automatically on startup if any workspace's model has changed.

On PostgreSQL, reindex execution is serialized per workspace by the tenant-pool advisory lock. On SQLite, the worker uses the model-owned reindex path directly under SQLite's single-tenant semantics.

The transactional dispatch outbox records lifecycle state, serialized worker arguments, and Loco's worker route in one database transaction before enqueueing. The `recover_worker_dispatches` Loco task retries uncorrelated initial dispatches and deferred lifecycle requeues after a cooldown, so an enqueue failure or process crash does not permanently strand those rows. Duplicate delivery is safe because lifecycle admission fences concurrent and terminal attempts.

Embedding execution failures are deferred and re-enqueued through the same class-specific Loco worker. If that re-enqueue fails, the outbox task retries it. Disabled or unconfigured embedding is an explicit no-op, deletion after enqueue is a successful no-op, and a stale snapshot is superseded by the newer entity write.

**Operational boundary for failed embedding jobs.** The `queue_job_lifecycles` row is authoritative once Loco deserializes the payload and its `lifecycle_id` passes admission. The outbox stores the original typed worker payload and queue route; a payload rejected before worker execution remains visible as a failed provider job and must be inspected through the selected queue backend.

Run `cargo loco scheduler` as a separate process in production. The configured task runs every minute and recovers pending dispatches and interrupted startup reindexes.

## Stripe webhooks

Set `YORISHIRO_STRIPE_WEBHOOK_SECRET` to enable inbound Stripe webhook processing.
The endpoint accepts only events whose raw request bytes pass Stripe's `Stripe-Signature` HMAC check and 300-second timestamp tolerance before JSON parsing.
The parsed event then crosses a private typed boundary into billing processing, so business logic never receives an unverified payload.
Loco extension points compose application routes and state, but they do not own provider-specific inbound cryptographic verification, so this boundary remains local to the Stripe controller.

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

## Environment configuration

Yorishiro uses Loco's `config/<environment>.yaml` layout.
`LOCO_ENV` selects the environment and defaults to `development`.
`LOCO_CONFIG_FOLDER` can point Loco at another directory containing the selected YAML file.
Package and Docker installations set `LOCO_ENV=production` and provide `production.yaml` in their configuration directory.

The YAML files use Loco's Tera `get_env` expressions for deploy-time values such as `DATABASE_URL`, `QUEUE_URL`, and embedding settings.
Application data supports SQLite and PostgreSQL.
Queue storage is selected independently through Loco's queue provider.
This release provides SQLite, PostgreSQL, and Redis-compatible queue providers.
An unconfigured development or production environment uses SQLite for both roles, so no external service is required.

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc` | `postgres://` URI for multi-tenant or vector search |
| `HOST` | `http://localhost` | Your server's hostname or address |
| `YORISHIRO_MAX_TENANTS` | `1` | Tenant cap; `0` allows unlimited tenants and disables the first-run setup wizard |
| `YORISHIRO_QUEUE_KIND` | Derived from `DATABASE_URL` | Loco queue provider: `Sqlite`, `Postgres`, or `Redis` in this release |
| `QUEUE_URL` | Provider default | Queue provider URI, including `redis://` or `rediss://` for the Redis-compatible provider |
| `YORISHIRO_EMBEDDING_BASE_URL` | Unset | Embeddings endpoint URL |
| `YORISHIRO_EMBEDDING_MODEL` | Unset | Embeddings model name |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | Embedding provider (`none`, `local`, or unset for local) |

When both the application and queue use SQLite, the queue URI must name a separate file.
The packaged default is `sqlite:///var/lib/yorishiro/yorishiro_queue.sqlite3?mode=rwc`, while `config/development.yaml` uses `sqlite://yorishiro_queue.sqlite3?mode=rwc`.
Boot rejects equivalent shared-file URIs after normalizing SQLite paths and ignoring connection query parameters.
Set `QUEUE_URL` explicitly when moving an existing installation or choosing a custom location.
PostgreSQL and Redis-compatible queues do not share the SQLite file restriction.

The application database and queue provider do not have to use the same backend.
For example, a PostgreSQL application database can use a Redis-compatible queue service by setting `YORISHIRO_QUEUE_KIND=Redis` and an appropriate `QUEUE_URL`.
Additional queue providers can be added without changing the application database configuration.

### Mailer

Email is not used by Yorishiro by default.
Add a `mailer:` block when required, then use `MAILER_HOST`, `MAILER_PORT`, `MAILER_USER`, and `MAILER_PASSWORD` for deploy-time overrides.

### Logging

Yorishiro uses the `LOG_LEVEL` environment variable to set logging verbosity (`debug`, `info`, `warn`, `error`). The `RUST_LOG` variable also works and takes priority over everything else.

## Workers and queues (background jobs)

A worker is an executable process that dequeues and runs background jobs.
A worker pool is a target group selected by `WorkerClass`: `TenantPrivate`, `Official`, or `Shared`.
The queue is durable job backlog storage, not a worker pool or a database connection pool.
Yorishiro requires `workers.mode: BackgroundQueue` in every environment configuration.
`ForegroundBlocking` and `BackgroundAsync` are rejected at configuration load and application boot.

Run the HTTP server and workers as separate processes.
The supported all-job worker command depends on how you deploy:

- **Package-based deployment (recommended):** use the wrapper installed at `/var/lib/yorishiro/worker-wrapper.sh`
  which discovers every registered tag at boot.  The systemd unit `yorishiro-worker.service` uses this path.

- **Source or Docker deployment:** use `cargo loco start --worker="$(cargo loco worker-tags)"`
  (source) or `/var/lib/yorishiro/worker-wrapper.sh` (Docker image copies the wrapper there).
  Both discover tags programmatically from the worker registry the process itself registers from, so they stay in sync when new worker classes or job types are added.

Pool-specific workers use an explicit tag:

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

### Dispatch seam inventory

Embedding sync and reindex enqueue paths use a small application-owned dispatch seam because their callers need deterministic tests for queue failure while Loco's `BackgroundWorker` extension point remains the production implementation.
The single production adapter lives at the existing Loco worker integration point, so worker registration, tags, queue modes, payloads, and execution remain unchanged.
Entity REST and MCP writes commit before they dispatch embedding sync, and the reindex endpoint commits its audit row before it dispatches.
The reindex audit detail intentionally changed from `{ "job_id": ... }` to `{ "workspace_id": ... }` as an observable contract change.
This preserves the append-only audit log and commit-before-dispatch ordering because the queue assigns `job_id` only during dispatch.
The reindex API response still returns the queue `job_id` for polling or operational tracking.

The scheduler, startup reindex detector, and workspace embedding-key update do not add another seam because they already delegate to the reindex enqueue helper.
Worker registration, worker execution, and the queue-routing probe remain Loco-specific because substitution there would test or replace framework behavior rather than application dispatch policy.
Infer-fill uses the same narrow seam in the Enterprise edition, while its durable job row is created before dispatch and marked failed when dispatch fails.

## Tenant reindex schedule

You can configure a deployment to automatically reindex every workspace under a tenant on a regular interval. The schedule runs through the normal reindex flow (same as the manual `reindex_embeddings` task), so it respects the same workspace provider and model version checks.

Configure the schedule through the API endpoint (`POST /api/identity/tenants/schedule` or the equivalent route), which takes an ISO 8601 duration (`P1D` for daily, `P1W` for weekly) and an optional IANA timezone name (default: UTC).
The five-minute interval sets the next `scheduled_for` time.
On each scheduler tick, any overdue `scheduled_for` runs, with no missed-run grace cutoff.

To run the scheduler on a fixed cron schedule, add a `scheduler:` entry to the selected `config/<environment>.yaml` that names the `TenantReindexScheduler` task. See the Loco documentation for the scheduler configuration format.

Run `cargo loco scheduler` as a separate process from the HTTP server and workers.
With PostgreSQL, any number of scheduler replicas may use the same database.
Each tick attempts the detached session-scoped advisory ownership lock `yorishiro:tenant-reindex-scheduler` and skips immediately when another replica owns it.
For PostgreSQL deployments, `DATABASE_URL` must connect directly to PostgreSQL or through a session-mode pooler.
Transaction-mode pooling is unsupported because this ownership lock is session-scoped and must remain on the same database session through schedule selection, the advancement commit, and every queue dispatch.
The detached connection remains held through schedule selection, advancement commit, and every queue dispatch.
It is explicitly unlocked and closed afterward.
Connection loss or abrupt process termination releases the lock automatically, so a surviving replica can take ownership on the next tick.
The ownership transaction reads due schedules and advances them before queue dispatch.
This is deliberate at-most-once dispatch protection: a scheduler crash cannot create duplicate queue jobs after the schedule commit, but a crash or queue failure after that commit can miss that tick.
The task attempts every due queue dispatch, then reports one aggregated task failure if any enqueue fails.
Those intervals are not retried, and the next scheduled interval remains the recovery point.
On PostgreSQL, the reindex worker's existing per-workspace advisory lock serializes actual reindex effects when manual, startup, or scheduled jobs overlap.
On SQLite, reindex workers use the direct single-tenant, model-owned reindex path and do not acquire that PostgreSQL advisory lock.

SQLite has no PostgreSQL-style advisory locks, so scheduler ownership uses a non-blocking OS file lock instead.
The lock file is adjacent to the effective SQLite database path and has the `.scheduler.lock` suffix.
Contending task processes skip immediately, and closing the handle or process termination releases the lock.
This acquisition happens inside the task, so it also covers scheduler jobs supplied through external Loco configuration.
In-memory SQLite scheduler operation is rejected because it has no deterministic adjacent file.
`YORISHIRO_SCHEDULER_REPLICAS` is diagnostic/compatibility metadata only and is never runtime authority.
Rolling deployments may overlap on both backends; the old owner must release before the new task proceeds.

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
HTTP access logs record only the request path, never its query string, so OAuth callback codes and state values are not logged.

## API key lifecycle

After setup, tenant owners and admins can create, list, and revoke workspace API keys through `/api/api-keys`.
`POST /api/api-keys` accepts a trimmed `name` from 1 to 100 characters and returns the full key only in that response.
`GET /api/api-keys` returns names, prefixes, identifiers, and creation times, but never hashes or plaintext secrets.
`DELETE /api/api-keys/{id}` revokes a key immediately and returns `404` when the identifier is absent or belongs to another tenant.
Revoking the key used for the request is permitted intentionally, so an administrator can revoke an old key without needing a second credential.
