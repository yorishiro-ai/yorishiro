# Configuration

**English** | [日本語](../ja/configuration.md)

Yorishiro is configured through environment variables. No config file is needed for the settings below.

## Server address and OAuth callback

Set `BINDING` to the interface the server listens on and `PORT` to its listening port (default `5150`).
`HOST` is the public URL used in links, not the listening address.
Set `YORISHIRO_OAUTH_REDIRECT_URI` to the public callback URL when using OAuth behind a reverse proxy or a different external port.
If it is unset, the callback defaults to `http://<BINDING>:<PORT>/auth/oauth/callback`; a wildcard binding such as `0.0.0.0` becomes `localhost`.
`YORISHIRO_BIND`, if set, overrides the fallback callback host and port but does not change where the server listens.

## Worker class admission limits

Community does not limit concurrent jobs by target.
Enterprise also leaves these limits off by default; set `YORISHIRO_PLAN_POLICY_ENABLED=true` to enable them.
Shared and Official limits apply across the deployment, while the tenant-private limit applies separately to each tenant.
All Official jobs share one running count, even when they belong to different plans.
Removing these limits does not change the number of workers or the embedding provider's own concurrency limits.
More concurrent jobs can increase database contention and model memory use without improving throughput.

| Variable | Description |
|---|---|
| `YORISHIRO_WORKER_CLASS_SHARED_LIMIT` | Maximum concurrent shared-class jobs when Enterprise plan policy is enabled (default: `1`, must be at least `1`) |
| `YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT` | Maximum concurrent tenant-private jobs when Enterprise plan policy is enabled (default: `1`, must be at least `1`) |
| `YORISHIRO_WORKER_OFFICIAL_LIMIT` | Optional maximum concurrent official jobs when Enterprise plan policy is enabled. When unset, Enterprise uses the plan value: Free `1`, Pro `4`, Team `8`. When set, it must be at least `1` |

Enterprise requires each configured limit to be a positive integer, even when plan policy is disabled.
Community ignores these three variables.
If you use a custom YAML configuration, omit `settings.worker_class_limits`; unknown settings prevent startup.

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

### Local model

Set `YORISHIRO_EMBEDDING_PROVIDER=local` to use a model that runs on your machine.

The only available model is `multilingual-e5-base` (768 dimensions), which works well for multiple languages including Japanese.

| Variable | Description |
|---|---|
| `YORISHIRO_LOCAL_MODEL` | Model name (default: `multilingual-e5-base`). An unrecognized value will fail startup |
| `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` | Maximum input length (default: `512`) |

**Downloading the model.** The model file is about 1 GiB and downloads on first use.
Subsequent starts use the cached copy.
You can also place both the model and tokenizer files in `models/<short_id>/` under the working directory.

If the download fails, restarting fixes it. The binary verifies the downloaded file against a built-in checksum.

### Changing embedding models

Supported embedding dimensions are `768`, `1024`, and `1536`.
Workspaces can use different models and supported dimensions.

If you switch to a different embedding model, existing embedded entities will still use vectors from the old model. You need to regenerate embeddings for affected workspaces:

```
cargo loco task reindex_embeddings workspace_id:<uuid>
```

This re-embeds every entity in the workspace with the new model. Until it finishes, new writes to that workspace will be rejected until the reindex completes.

You can also queue a reindex via the API: `POST /api/migration-jobs/reindex` (requires Migration scope). A reindex also runs automatically on startup if any workspace's model has changed.

Failed embedding jobs are not retried automatically.
Use `resync_embeddings` or `reindex_embeddings` to recover after fixing the cause.
If a job has an invalid payload, check the queue provider's failed jobs; its lifecycle row can remain `queued`.
For an embedding execution failure, check the lifecycle row's `error` field.

## Stripe webhooks

Set `YORISHIRO_STRIPE_WEBHOOK_SECRET` to enable inbound Stripe webhook processing.
The endpoint verifies `Stripe-Signature` against the raw request with a 300-second timestamp tolerance before processing an event.

## Search token quota

| Variable | Description |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | Tokens per minute a workspace can spend on search (default: `100000`). Charged per query, shared between API and MCP. The default is very high and only matters for runaway automated queries. Japanese text is charged at roughly half the token cost of English with the default estimate, so the quota allows more Japanese queries per minute |

## Database load guard

The PostgreSQL load guard is disabled by default because it changes the deployment-wide maintenance state automatically.

Set `YORISHIRO_DB_LOAD_THRESHOLD` to the active-connection count that should be treated as busy to enable it.
The guard samples `pg_stat_activity` every 5 seconds by default and changes to read-only only after the threshold remains crossed for 30 seconds.
It returns to normal service after the same quiet period, but only when the guard itself set read-only mode.
An operator-owned read-only or full-lock state is never cleared by the guard.
The guard uses the PostgreSQL identity pool and is a no-op on SQLite.
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
SQLite, PostgreSQL, and Redis-compatible queue providers are available.
An unconfigured development or production environment uses SQLite for both roles, so no external service is required.

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc` | `postgres://` URI for multi-tenant hosting |
| `HOST` | `http://localhost` | Your server's hostname or address |
| `YORISHIRO_MAX_TENANTS` | `1` | Tenant cap; `0` allows unlimited tenants and disables the first-run setup wizard |
| `YORISHIRO_MCP_ALLOWED_HOSTS` | Unset | Comma-separated exact `Host` values allowed by `/mcp` in addition to `localhost`, `127.0.0.1`, and `::1`. Wildcards are rejected. |
| `YORISHIRO_QUEUE_KIND` | Derived from `DATABASE_URL` | Loco queue provider: `Sqlite`, `Postgres`, or `Redis` |
| `QUEUE_URL` | Provider default | Queue provider URI, including `redis://` or `rediss://` for the Redis-compatible provider |
| `YORISHIRO_EMBEDDING_BASE_URL` | Unset | Embeddings endpoint URL |
| `YORISHIRO_EMBEDDING_MODEL` | Unset | Embeddings model name |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | Embedding provider (`none`, `local`, or unset for local) |

When both the application and queue use SQLite, the queue URI must name a separate file.
The packaged default is `sqlite:///var/lib/yorishiro/yorishiro_queue.sqlite3?mode=rwc`, while `config/development.yaml` uses `sqlite://yorishiro_queue.sqlite3?mode=rwc`.
Boot rejects equivalent shared-file URIs after normalizing SQLite paths and ignoring connection query parameters.
Set `QUEUE_URL` explicitly when moving an existing installation or choosing a custom location.
PostgreSQL and Redis-compatible queues do not share the SQLite file restriction.

Supported and CI-tested configurations are SQLite with a SQLite queue, PostgreSQL with a PostgreSQL queue, and either database with a Redis-compatible (Valkey) queue.
Crossed SQL configurations (SQLite database with PostgreSQL queue, or PostgreSQL database with SQLite queue) are forbidden and rejected before boot with `ValidationFailed` (`validation_failed`).
The error message explains the invalid pair in English and Japanese and asks for a matching SQL queue or Valkey.
Without a queue override, SQLite defaults to a SQLite queue and PostgreSQL defaults to a PostgreSQL queue.
RabbitMQ and SQS are not implemented; future support is restricted to the enterprise edition with a PostgreSQL database, never SQLite or the community edition.
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
Packages and Docker Compose start a worker for the shared target only, suitable for a personal installation.
For source deployments run `cargo loco start --worker=worker-target:shared`; the installed binary uses `yorishiro start --worker=worker-target:shared`.
Operators who need Official or tenant-private work must start additional processes explicitly:

```console
cargo loco start --worker=worker-target:official
cargo loco start --worker=worker-target:tenant:0198f3a0-0000-7000-8000-00000000000a
```

Use the tenant's canonical lowercase hyphenated UUID, not a workspace ID.
Each job has exactly one composite `worker-target:<target>` tag.
Redis uses a named queue with the same name for each target, and each worker polls only its selected target queues.
Query embedding and Enterprise infer-fill use the shared target; a query-only or infer-fill-only worker selection is not available.
A process may select several targets with comma-separated tags, but that does not reserve capacity for any target.
A bare `--worker`, `--all`, or `--server-and-worker` is rejected; plain `start` continues to run only the server.
Configure any automatic start of Official or tenant workers in your own process manager; no tag-discovery wrapper or per-profile configuration is supplied.

**When changing worker routing, stop job producers and drain queued, running, and retrying jobs with the matching workers before starting workers with the new routing.**
Keep the matching Redis queues and workers available until both the application outbox and provider queues are drained.
Do not run producers and workers with different routing at the same time; jobs can remain unprocessed.
The startup check detects unfinished dispatches in the application outbox, but it cannot detect jobs in provider queues or jobs enqueued concurrently.
A tenant Redis queue is created for each tenant target when jobs arrive, and deleting a tenant does not automatically remove its Redis queue.
Monitor queue growth and remove an obsolete tenant queue only after its jobs have been drained and the tenant is no longer served.

Every worker process needs the same queue configuration and embedding or inference configuration that its jobs require.
Workers may run on the same host as the HTTP server or on a separate server.
Separate-server workers must use the same `QUEUE_URL` as the server.
When that URL names a shared SQLite file, use an absolute path because a relative path is resolved independently on each host and is not a shared queue.
With PostgreSQL, multiple worker processes can dequeue in parallel.
With SQLite, dequeueing is serialized, while durable queue recovery still works.

`YORISHIRO_QUEUE_WORKERS` controls the number of concurrent dequeue loops in one worker process.
It defaults to 1 for a SQLite queue and 2 for PostgreSQL and Redis-compatible queues.
SQLite has a single writer, so more workers on a SQLite queue add contention, not throughput, and boot logs a warning when you set them.
`DB_MAX_CONNECTIONS` defaults to 10 for a SQLite database and 100 for PostgreSQL, and boot warns when a SQLite pool exceeds 16.
Crossed SQL configurations fail validation rather than emitting an advisory.
SQLite with a Redis-compatible queue remains supported, but boot advises keeping workers on the database's host because remote workers cannot open the SQLite file and SQLite has no named locks.
Throughput scales most with PostgreSQL for application data and a Redis-compatible queue (Valkey), then with PostgreSQL for both, and least with SQLite for both.
`YORISHIRO_QUEUE_REAPER_AGE_MINUTES` controls how long a job may remain processing before the reaper recovers it.

## Tenant reindex schedule

You can configure a deployment to automatically reindex every workspace under a tenant on a regular interval. The schedule runs through the normal reindex flow (same as the manual `reindex_embeddings` task), so it respects the same workspace provider and model version checks.

Configure the schedule through the API endpoint (`POST /api/identity/tenants/schedule` or the equivalent route), which takes an ISO 8601 duration (`P1D` for daily, `P1W` for weekly) and an optional IANA timezone name (default: UTC).
The interval accepts weeks, days, hours, minutes and seconds (for example `P1D`, `P1W`, `PT6H`) and must be at least `PT5M`; years and months are rejected.
Repeated units such as `P1D1D` are rejected.
For stored schedules with intervals shorter than five minutes, the ticker uses five minutes; it falls back to one day when an interval is unreadable or its next timestamp overflows.
The first `scheduled_for` is one interval after the schedule is set, and each scheduler tick that runs it sets the next one an interval after that tick.
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
