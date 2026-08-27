# Configuration Reference

**English** | [日本語](ja/configuration.md)

This is not a full settings reference: it covers the embedding provider, the per-workspace search token quota, and `config/production.yaml`'s queue tuning.
Every variable listed here is read directly from the environment; there is no `config.yml`-style file for these settings on this branch.

## Embedding provider

`build_embedding_provider` (`src/services/embedding/mod.rs`) selects and configures the provider used for both writing embeddings (`sync_embedding`) and search (`GET /api/search`, the `search_entities` MCP tool).

| Variable | Description |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` selects the local ONNX provider (see below). Anything else, or unset, selects the OpenAI-compatible provider |
| `YORISHIRO_EMBEDDING_BASE_URL` | OpenAI-compatible provider only. Base URL of an OpenAI-compatible embeddings endpoint (LM Studio, Ollama, vLLM, or real OpenAI), e.g. `http://localhost:11434`. Required together with `YORISHIRO_EMBEDDING_MODEL`: if either is unset, boot proceeds with no embedding backend configured rather than failing, and every embed call fails at request time with `ProviderUnreachable` instead |
| `YORISHIRO_EMBEDDING_MODEL` | OpenAI-compatible provider only. Model name sent in the `model` field of the embeddings request. Also stamped onto a workspace at creation time as the model it was embedded with; unset, a workspace is stamped `unconfigured` |
| `YORISHIRO_EMBEDDING_API_KEY` | OpenAI-compatible provider only. Bearer token sent to `YORISHIRO_EMBEDDING_BASE_URL`. Empty by default, which is correct for a local server (LM Studio, Ollama) that doesn't check one |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | Expected vector width (default: `768`). Every vector in a deployment must share this width; the local ONNX provider verifies it at startup with a probe inference, the OpenAI-compatible provider verifies it per response |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | OpenAI-compatible provider only. `true` includes a `dimensions` field in the embeddings request. Default `false`, since some OpenAI-compatible implementations (vLLM, Ollama, LM Studio) reject a `dimensions` field they don't recognise |

### A workspace's own embedding provider (paid edition)

`PUT /hosted/workspace/embedding-key` points one workspace's own embedding work at a different provider than the deployment-wide one above, instead of every workspace sharing `YORISHIRO_EMBEDDING_BASE_URL`.
Not part of the base edition: this is the same split as `PUT /hosted/workspace/llm-key`, which assigns LLM inference credentials per workspace already.
Which workspace uses which compute backend is a paid-edition decision.

| Field | Description |
|---|---|
| `base_url` | An OpenAI-compatible embeddings endpoint, e.g. `https://api.openai.com/v1` |
| `model` | Model name sent in the embeddings request |
| `api_key` | Bearer token. Stored, and never returned by `GET`: only `base_url`, `model`, `dimensions` and whether one is configured come back |
| `dimensions` | The vector width this provider produces |
| `send_dimensions_param` | `true` includes a `dimensions` field in the embeddings request; default `false`, matching `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` above |

A workspace with nothing configured here keeps using the deployment-wide provider (`YORISHIRO_EMBEDDING_BASE_URL` etc.), so setting nothing is the same as before this endpoint existed.
`DELETE /hosted/workspace/embedding-key` returns a workspace to that deployment default.

`PUT` refuses a `dimensions` value that does not match the workspace's own stamped vector width (the `embedding_dimensions` recorded when the workspace was created) with `422`, before storing anything: assigning a provider whose output width does not match the vectors already on disk would otherwise only be discovered on the next entity write, when `sync_embedding`'s own write-time check (`services/embedding/sync.rs`) rejects it.
Both checks exist and neither replaces the other: the write-time check still runs regardless of how a workspace ended up with a mismatched provider, but this one surfaces the same mistake immediately, at the point an operator makes it.

There is no caching: an assignment made through this endpoint takes effect on the very next request that resolves an embedding provider for that workspace (search, embedding sync), not after some delay or a restart.

### Local ONNX provider (`YORISHIRO_EMBEDDING_PROVIDER=local`)

Runs a BERT-family ONNX model in-process, with no external embedding service.

| Variable | Description |
|---|---|
| `YORISHIRO_ONNX_MODEL_PATH` | Path to the `.onnx` model file (default: `models/model.onnx`). Not bundled with the repository or fetched automatically; boot fails with a message naming both this path and `YORISHIRO_ONNX_TOKENIZER_PATH` if either file is missing |
| `YORISHIRO_ONNX_TOKENIZER_PATH` | Path to the tokenizer's `tokenizer.json` (default: `models/tokenizer.json`). Same missing-file behaviour as `YORISHIRO_ONNX_MODEL_PATH` |
| `YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH` | Maximum token count per input, truncating longer text (default: `512`) |
| `YORISHIRO_ONNX_POOLING` | `mean` (default) or `last_token` (also accepts `last-token`/`lasttoken`). An unrecognized value is rejected at boot rather than silently falling back to `mean`: reading a model with the wrong pooling doesn't fail, it just returns worse vectors, and defaulting quietly on a typo would hide exactly that degradation |
| `YORISHIRO_ONNX_QUERY_INSTRUCTION` | Instruction text embedded into a search query only, never into a stored document, for asymmetric models that expect one on the query side (rendered as `Instruct: {instruction}\nQuery:{text}`). Unset by default, which makes this exactly a plain `embed` call, the right behaviour for a symmetric model. An empty string is treated the same as unset, not as "prefix with nothing": clearing the variable is how an operator turns the instruction back off |

Building with this provider compiled in pulls in the `ort` crate, whose default `download-binaries` feature fetches an onnxruntime binary from `cdn.pyke.io` at build time; point `ORT_LIB_LOCATION` at a pre-provisioned onnxruntime if the build environment must be closed off.

## Search token quota

| Variable | Description |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | Tokens a workspace may spend on search per minute (default: `100000`). Charged once per query, before embedding, whether the request arrives at `GET /api/search` or through the `search_entities` MCP tool: one shared budget per workspace, not one per protocol. A query over budget gets HTTP `422` (`validation_failed`) instead of running. The default is high enough that ordinary use never reaches it; it exists to bound a runaway agent, not to ration normal traffic |

Search is metered in tokens rather than requests because that's what a query costs the embedding model; entity writes stay on request counts, since counting a large body is itself expensive.
The token count for a query comes from `EmbeddingProvider::count_tokens`, which the local ONNX provider overrides to the tokenizer's exact count and every other provider defaults to a byte-length estimate (`text.len() / 4`, rounded up).
That estimate is calibrated for English, where roughly 4 bytes make one token; Japanese text is roughly 3 bytes per character and tokenizes at roughly one token per character, so the estimate returns under half the token count a real tokenizer would report for the same Japanese query.
In other words, outside the local ONNX provider, a Japanese search query is charged against the budget at well under its real cost: `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` admits noticeably more Japanese-language search traffic per minute than English before this quota starts returning `422`.
Size the budget with that skew in mind if the deployment's search traffic is mostly Japanese and not running the local ONNX provider.

## Queue backend and tuning (`config/development.yaml`, `config/production.yaml`)

`queue.kind` is switchable at boot, since loco-rs ships three queue providers (Postgres, `SQLite`, Redis/Valkey, `QueueConfig`'s `#[serde(tag = "kind")]` variants) and each needs a different set of fields (Redis alone takes `queues`; Postgres/`SQLite` share the SQL-pool knobs but point at different URIs). Both `development.yaml` and `production.yaml` template a whole alternative `queue:` block per `kind` (a Tera `<% if %>`/`<% elif %>`/`<% endif %>`) rather than templating individual fields inside one fixed shape.

| Variable | Description |
|---|---|
| `YORISHIRO_QUEUE_KIND` | `Sqlite` (default in `development.yaml`, matching that file's database default so an unconfigured start needs no Postgres), `Postgres`, or `Redis`. Booting with `Redis` needs the `worker_redis` Cargo feature compiled in (enabled in this workspace's `Cargo.toml`) or startup fails with "No queue provider feature was selected and compiled" |
| `QUEUE_URL` | The queue backend's own connection URI. In `development.yaml` it defaults to a SQLite file of its own on the `Sqlite` kind, and to `DATABASE_URL` on the `Postgres` kind; `production.yaml` requires it explicitly with no default on every kind, matching that file's own no-silent-fallback convention |
| `YORISHIRO_QUEUE_WORKERS` | How many workers dequeue jobs in parallel (default: `2`). Postgres claims a row with `FOR UPDATE SKIP LOCKED`, so raising this genuinely adds parallelism on that backend; `SQLite`'s `BEGIN IMMEDIATE` serializes every dequeue regardless of this number |
| `YORISHIRO_QUEUE_REAPER_AGE_MINUTES` | Minutes a job may sit in `processing` before the reaper requeues it as `Queued` (default: `30`). Loco's own reaper is opt-in and off by default: without it, a job a worker died on while it was running (a crash, a forced kill) stays `processing` forever, since nothing else moves a job out of that state, `fail_job` only runs when `perform` itself returns an error. Set this above the longest a healthy job can legitimately take, or the reaper requeues work that is still genuinely in progress |

`development.yaml` enables the same reaper with fixed values (`num_workers: 2`, `age_minutes: 10`) rather than reading `YORISHIRO_QUEUE_WORKERS`/`YORISHIRO_QUEUE_REAPER_AGE_MINUTES`, since a local development environment has no reason to tune them per deployment; `production.yaml` reads both.
`config/test.yaml` has no `queue:` block at all (`docs`/`.claude/rules/testing.md` covers why), so none of this applies there.

`config/sqlite.yaml` (the manual-verification SQLite tier, `docs/sqlite.md`) also configures `queue: kind: Sqlite` with `workers.mode: BackgroundQueue`, the same as the other two environments. loco-rs's `SQLite` queue provider (`bgworker::sqlt`) opens its own `sqlx::SqlitePool`, independent of the application's own `SQLite` connection, so it is a genuinely separate pool against the same or a different file, not routed through `db.rs`'s RLS-aware pool (`SQLite` has no RLS to be aware of). Measured directly against a real file: a concurrent write from the queue pool while the application holds an open write transaction on the same file waits out `sqlx`'s own 5-second default `busy_timeout` and succeeds once that transaction releases the lock, rather than failing. In this codebase specifically, the embedding-sync enqueue call only runs after the request's own write transaction has already committed, so this scenario does not arise from a single request; it would only matter for a genuinely concurrent second request racing the first's still-open transaction, and `content_entities::create` is one fast `INSERT`, well under the 5-second budget.

## Running workers on a separate process or host

`cargo loco start --worker[=tag1,tag2]` (or the equivalent `yorishiro_core-cli`/`yorishiro_server` invocation, both binaries share loco-rs's own CLI) runs only the queue worker loop, no HTTP server, in the current process. `--worker=worker-class:official` restricts that process to jobs carrying that tag (`WorkerClass::tag()`, `src/workers/embedding_sync.rs`). A separate process, on a separate host, needs nothing beyond pointing its own config at the same `queue.uri`/`QUEUE_URL` and `database.uri`/`DATABASE_URL` the server uses — no additional networking layer, shared secret, or node-registration step.

**`--worker` with no value does not take every job.** Confirmed against `loco-rs` 1.1.0's own dequeue SQL (shared shape across the Postgres/`SQLite`/Redis queue providers): an empty tag list means "untagged jobs only", not "every job regardless of tag". Every job this deployment enqueues always carries exactly one `worker-class:*` tag (`workers::embedding_sync::enqueue_for_class`), so it is never untagged — a bare `--worker` process here dequeues none of these jobs, ever, not "the ones nothing else claimed". A deployment that wants one process to cover every class must name every tag explicitly: `--worker=worker-class:tenant-private,worker-class:official,worker-class:shared`. There is no wildcard/catch-all flag in `loco-rs` 1.1.0.

**A worker-only process still needs the server's full config, not just the queue connection.** `Hooks::after_context` (`src/app.rs`) runs unconditionally for every `StartMode` loco-rs has, including `--worker`-only: it builds the RLS-aware tenant pool and the migration-role identity pool against `DATABASE_URL` regardless of whether the process ever serves a request, and it fails boot outright if the embedding provider is misconfigured ("Boot fails loudly ... rather than deferring the error to the first search," the same doc comment names this deliberate). Every `WorkerClass` worker type's `perform` genuinely uses both: it reads `ctx.db` to re-fetch the entity and calls `resolve_embedding_provider`, which needs the same `YORISHIRO_EMBEDDING_*` variables (or a workspace's own assignment) the server needs. A worker node configured with only a queue connection and nothing else fails at boot, not silently: the operator error this guards against is assuming "the worker only talks to the queue" and skipping the rest of the config.

**At least one process must stay subscribed to every `WorkerClass`'s tag, named explicitly.** If every running worker process is tag-restricted and no process names all three (`worker-class:tenant-private`, `worker-class:official`, `worker-class:shared`), whichever class none of them cover queues forever in `pg_loco_queue`/`sqlt_loco_queue` with nothing to dequeue it. A deployment adding a dedicated `worker-class:official` node must keep (or add) at least one process still running with all three tags named (not bare `--worker`, see above) to cover `Shared` and any other class that node doesn't.

**What actually parallelizes across multiple worker processes/hosts depends on the queue backend**, the same distinction `YORISHIRO_QUEUE_WORKERS`'s own row above already draws for `num_workers` within one process: Postgres's `pg_loco_queue` dequeue uses `FOR UPDATE SKIP LOCKED`, so multiple processes (on one host or several) genuinely dequeue different jobs concurrently. `SQLite`'s `sqlt_loco_queue` dequeue uses `BEGIN IMMEDIATE`, which takes the file's one write lock, so a second process pointed at the same `SQLite` file serializes behind the first rather than adding real parallelism — running more than one worker process against a `SQLite`-backed queue buys resilience (a second process to pick up work if the first dies) but not throughput.

### A workspace's own worker-class assignment (paid edition)

`PUT /hosted/workspace/worker-class` pins one workspace's embedding-sync jobs to `tenant_private` or `official` compute instead of the shared pool every workspace uses by default.
Not part of the base edition: which compute a tenant's jobs run on is the same paid-edition decision that already assigns LLM/embedding credentials per workspace (`PUT /hosted/workspace/llm-key`, `PUT /hosted/workspace/embedding-key`).

| Field | Description |
|---|---|
| `worker_class` | One of `tenant_private`, `official`, `shared` |

A workspace with nothing configured here keeps its jobs `shared`, so setting nothing is the same as before this endpoint existed.
`DELETE /hosted/workspace/worker-class` returns a workspace to `shared`.
No caching: an assignment made through this endpoint takes effect on the very next job enqueued for that workspace, not after some delay or a restart.
Assigning a workspace to `tenant_private`/`official` has no effect on its own until a worker process actually subscribes to that tag ("Running workers on a separate process or host" above) — a workspace can be assigned a class with no node running it yet, and its jobs simply queue until one does.
