# Configuration Reference

**English** | [日本語](ja/configuration.md)

This is not a full settings reference: it covers the embedding provider, the per-workspace search token quota, and `config/production.yaml`'s queue tuning.
Every variable listed here is read directly from the environment; there is no `config.yml`-style file for these settings.

## Embedding provider

`build_embedding_provider` (`src/services/embedding/mod.rs`) selects and configures the provider used for both writing embeddings (`sync_embedding`) and search (`GET /api/search`, the `search_entities` MCP tool).

### When an embedding is generated

Creating or replacing an entity queues a background job that generates its vector, on both transports: `POST /api/entities` and `PUT /api/entities/{id}`, and the `create_entity` and `update_entity` MCP tools.
The job is enqueued only after the write's own transaction has committed, so the embedding provider round trip never adds its latency to the write.

**`import_jsonl` is the exception, on either transport.** A restored backup's entities are written with no embedding job queued, so they stay `embedding IS NULL` and are reachable only through the `pg_trgm` fuzzy fallback until something fills them in.
Run `cargo loco task resync_embeddings workspace_id:<uuid>` after an import; the same command recovers entities whose background sync failed against an embedding provider outage that outlasted the job's own retries.

An entity with a NULL embedding produces no error at any point.
Search simply returns worse results for it, so this is worth checking after a restore rather than waiting for a report.

`resync_embeddings` and `reindex_embeddings` (below) are both PostgreSQL-only: `content_entities` has no `embedding` column at all on SQLite, since vector search is not ported to that backend (see `docs/sqlite.md`).

### Moving a workspace between embedding models

Every write checks the workspace's own stamp (`identity_workspaces.embedding_model`/`embedding_dimensions`) against the configured provider before writing a vector, and refuses the write with `422` on a mismatch (`sync_embedding`'s write-time model check, `services/embedding/sync.rs`).
This exists because `content_entities.embedding` is a single fixed-width column: two different models can happen to produce the same width (nomic-embed-text-v1.5 and multilingual-e5-base both produce 768 dimensions, as of this writing) and Postgres has no way to tell which model actually produced a given vector, only how wide it is.
Without this check, pointing a deployment's `YORISHIRO_EMBEDDING_PROVIDER`/`YORISHIRO_LOCAL_MODEL` at a different model than a workspace was embedded with would silently write vectors that are not comparable to the ones already stored, degrading search with no error anywhere.
A workspace that has no stamp of its own (both `embedding_model` and `embedding_dimensions` are `NULL`) inherits the deployment default at write time, and `sync_embedding` stamps the first successful embed with the current provider's model and dimensions.
A tenant can also set its own `embedding_model`/`embedding_dimensions` as a default for all its workspaces; workspace stamps take priority over tenant defaults, which take priority over the deployment default.

**The procedure is, in order: change the provider configuration and restart, then run `reindex_embeddings` against every affected workspace.**
Restarting with `YORISHIRO_LOCAL_MODEL` (or `YORISHIRO_EMBEDDING_MODEL`, for the OpenAI-compatible provider) already pointed at the new model, before any workspace is reindexed, is expected and correct, not a step to avoid.
From that restart until each workspace's reindex finishes, every ordinary write to that workspace is refused by the check above: this is the guard doing its job, not a failed upgrade, and it is the reason the check exists in the first place, to make the mismatch loud rather than let the deployment write incomparable vectors silently.
Run `cargo loco task reindex_embeddings workspace_id:<uuid>` once per affected workspace to close that window: it re-embeds every entity with the now-configured provider and only then restamps `embedding_model`/`embedding_dimensions` to match, and only if every entity succeeded, at which point ordinary writes to that workspace pass the check again.
Unlike `resync_embeddings`, which only fills entities with no vector yet, `reindex_embeddings` re-embeds every entity in the workspace regardless of whether it already has one, since the whole point is replacing vectors from the old model rather than filling gaps.
A failed or partial reindex run leaves the workspace's old stamp in place, so the write-time check keeps refusing ordinary writes to it until the task is re-run and completes fully; re-running is safe, since it re-embeds every entity again regardless of an earlier partial attempt.
An entity written while a reindex is in flight goes through the ordinary guarded path against the workspace's still-old stamp: it succeeds if the deployment's provider still matches that stamp (impossible after the restart above, since the provider has already moved) and is otherwise refused, the same as every other write in that window, resolving once the reindex completes and the stamp catches up.

| Variable | Description |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` selects the local in-process provider (see below). Anything else, or unset, selects the OpenAI-compatible provider |
| `YORISHIRO_EMBEDDING_BASE_URL` | OpenAI-compatible provider only. Base URL of an OpenAI-compatible embeddings endpoint (LM Studio, Ollama, vLLM, or real OpenAI), e.g. `http://localhost:11434`. Required together with `YORISHIRO_EMBEDDING_MODEL`: if either is unset, boot proceeds with no embedding backend configured rather than failing, and every embed call fails at request time with `ProviderUnreachable` instead |
| `YORISHIRO_EMBEDDING_MODEL` | OpenAI-compatible provider only. Model name sent in the `model` field of the embeddings request. Every provider stamps its own model this way on the first successful entity embed; the local provider stamps the model it loaded regardless of this variable. Workspaces start with no stamp (`NULL` for both `embedding_model` and `embedding_dimensions`), and `sync_embedding` records the stamp on the first successful embed. A tenant can also set its own `embedding_model`/`embedding_dimensions` as a default for all its workspaces |
| `YORISHIRO_EMBEDDING_API_KEY` | OpenAI-compatible provider only. Bearer token sent to `YORISHIRO_EMBEDDING_BASE_URL`. Empty by default, which is correct for a local server (LM Studio, Ollama) that doesn't check one |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | Expected vector width (default: `768`). Every vector in a deployment must share this width; the local provider verifies it at startup with a probe inference, the OpenAI-compatible provider verifies it per response |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | OpenAI-compatible provider only. `true` includes a `dimensions` field in the embeddings request. Default `false`, since some OpenAI-compatible implementations (vLLM, Ollama, LM Studio) reject a `dimensions` field they don't recognise |

### A workspace's own embedding provider (enterprise edition)

`PUT /api/workspace/embedding-key` points one workspace's own embedding work at a different provider than the deployment-wide one above, instead of every workspace sharing `YORISHIRO_EMBEDDING_BASE_URL`.
Not part of the base edition: this is the same split as `PUT /api/workspace/llm-key`, which assigns LLM inference credentials per workspace already.
Which workspace uses which compute backend is a enterprise-edition decision.

| Field | Description |
|---|---|
| `base_url` | An OpenAI-compatible embeddings endpoint, e.g. `https://api.openai.com/v1` |
| `model` | Model name sent in the embeddings request |
| `api_key` | Bearer token. Stored, and never returned by `GET`: only `base_url`, `model`, `dimensions` and whether one is configured come back |
| `dimensions` | The vector width this provider produces |
| `send_dimensions_param` | `true` includes a `dimensions` field in the embeddings request; default `false`, matching `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` above |

A workspace with nothing configured here keeps using the deployment-wide provider (`YORISHIRO_EMBEDDING_BASE_URL` etc.), so a deployment that assigns nothing is unaffected by this endpoint.
`DELETE /api/workspace/embedding-key` returns a workspace to that deployment default.

`PUT` refuses a `dimensions` value that does not match the workspace's own stamped vector width (the `embedding_dimensions` recorded when the workspace was created) with `422`, before storing anything: assigning a provider whose output width does not match the vectors already on disk would otherwise only be discovered on the next entity write, when `sync_embedding`'s own write-time check (`services/embedding/sync.rs`) rejects it.
Both checks exist and neither replaces the other: the write-time check still runs regardless of how a workspace ended up with a mismatched provider, but this one surfaces the same mistake immediately, at the point an operator makes it.

There is no caching: an assignment made through this endpoint takes effect on the very next request that resolves an embedding provider for that workspace (search, embedding sync), not after some delay or a restart.

### Local provider (`YORISHIRO_EMBEDDING_PROVIDER=local`)

Runs a model in-process from a `safetensors` checkpoint, with no external embedding service.
Two models are selectable, both 768-dimensional: `nomic-embed-text-v1.5` (`candle-transformers`' `nomic_bert`, a BERT variant with rotary position embeddings and a SwiGLU MLP) and `multilingual-e5-base` (`candle-transformers`' `xlm_roberta`).
Pooling is always mean pooling for either model: that is what both were trained with, so there is nothing left for a pooling setting to select between.

`multilingual-e5-base` is trained on query/passage-prefixed text: a search query is embedded as `"query: " + text` and a stored document as `"passage: " + text`, applied automatically by this provider and invisible to callers.
`nomic-embed-text-v1.5` uses no such prefix.

| Variable | Description |
|---|---|
| `YORISHIRO_LOCAL_MODEL` | Which model to load: `nomic-embed-text-v1.5` or `multilingual-e5-base`. Unset defaults to `multilingual-e5-base`, since this codebase's search and recall are not English-only. An unrecognized value fails startup naming the valid ones, rather than silently falling back to the default |
| `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` | Maximum token count per input, truncating longer text (default: `512`, unchanged regardless of which model is selected). Must fit the selected model's own upper bound (`8192` for nomic-embed-text-v1.5, `512` for multilingual-e5-base); the default already satisfies both, so it needs no per-model adjustment on its own |

Switching `YORISHIRO_LOCAL_MODEL` on a deployment that already has embedded workspaces does not itself move any workspace to the new model: the write-time model check ("Moving a workspace between embedding models" above) refuses writes for a workspace still stamped with the old one until `reindex_embeddings` runs against it.

These variables were named `YORISHIRO_ONNX_*` before this provider moved from `ort`/ONNX to candle.
A deployment that still has an old name set fails to start, but the message differs by what became of that variable.
`YORISHIRO_ONNX_MODEL_PATH` and `YORISHIRO_ONNX_TOKENIZER_PATH` were renamed: the message names both the old and new variable, since a stale one of these would otherwise go unnoticed while this provider's normal resolution ran instead.
`YORISHIRO_LOCAL_MODEL_PATH` and `YORISHIRO_LOCAL_TOKENIZER_PATH` were then removed altogether in a later release: the operator-chosen path was structurally unbound from the model identifier (which comes from `YORISHIRO_LOCAL_MODEL`), so a mismatch could never be detected at write time, and the separation was removed entirely.
`YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH` was renamed to `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH`.
`YORISHIRO_ONNX_POOLING` and `YORISHIRO_ONNX_QUERY_INSTRUCTION` were removed outright rather than renamed (see above and "Known gap" in this repository's own history for why), so a stale one of these carries no such risk: nothing reads it, and there is no other behaviour left for it to have selected. The message says so rather than claiming the same risk as the renamed variables.
Remove every old variable (or rename it to its replacement, for the renamed ones) before starting again.

#### Fetching the model

The model and tokenizer are not in the repository: nomic-embed-text-v1.5's model file is about 522 MiB, multilingual-e5-base's about 1.04 GiB.
When nothing is at the default `models/<short_id>/` path, both files are fetched on first use into `$HOME/.cache/yorishiro/models/<short_id>/` and verified against a SHA256 built into the binary.
That directory is also where later starts look first, so only the first one pays the download.
`<short_id>` scopes both the default and cache paths to the selected model (`nomic-embed-text-v1.5` or `multilingual-e5-base`), so switching `YORISHIRO_LOCAL_MODEL` never finds the other model's cached files at its own path.

Verification happens at download time, not at every read.
A file only reaches that directory by passing both checks and then being moved into place atomically, so a later start treats what is already there as the product of that earlier verified download rather than hashing it again on every start.
It does check the cached file's size, which is free and catches a truncated file; one of the wrong size is removed and fetched again, so that case repairs itself rather than being loaded as though it were sound.
Files you supply yourself, at `models/<short_id>/`, are not checked at all: no digest can be pinned for a model of your choosing, and it may deliberately be a different one.
That trust extends to identity, not just bytes: this provider reports `def.id` (the catalog model named by `YORISHIRO_LOCAL_MODEL`) regardless of what the supplied checkpoint actually is, so a custom `nomic-embed-text-v1.5` checkpoint holding a different model's weights is stamped onto a workspace and compared by the write-time model check as if it really were nomic-embed-text-v1.5.
Both models are pinned to a fixed revision rather than a branch, so the bytes behind their digests cannot change underneath a deployment.

The download blocks whatever triggered it, and the log line before it says so.
That is usually a server start, but not always: `cargo loco task create_workspace`, `cargo loco task resync_embeddings`, and `cargo loco task reindex_embeddings` build an embedding provider too, so any of them can be what pulls the model on a fresh machine, and a task that appears to be sitting still is most likely doing this.
Each variable defaults independently, so setting only one leaves the other at its `models/<short_id>/` default.

Placing the files at the default `models/<short_id>/` path by hand also works and is never overwritten.
Both must be there: if exactly one is present the start fails, naming the file that is there and the one that is missing, rather than fetching around it.
A lone file at that path is a half-finished setup, and quietly ignoring it would embed with a different model than the one deliberately placed there, which can disagree with the vectors already indexed while everything still looks healthy.

Two failures are treated differently, on whether starting again could help.
A download that fails, or whose bytes do not match the expected digest, fails the start: a network outage is transient, so a supervisor configured to restart retries it and the deployment heals itself, while a digest mismatch at a pinned revision means corruption or tampering and is exactly what verification exists to stop.
If `HOME` does not resolve at all there is nowhere to fetch to, no restart changes that, and the deployment starts with no embedding provider, logging a message naming both path variables; search and recall then error until one is set.

A download that is killed partway leaves a `.partial.` file behind in the cache directory, which a later start removes once it has gone six hours without being written to.
The age requirement is what keeps that sweep away from a download still in progress, so two processes starting together do not delete each other's work.

This provider builds a fully statically linked binary: `candle-core`/`candle-nn`/`candle-transformers` need no prebuilt runtime binary and no dynamic OpenSSL link, unlike the ONNX-based provider this replaced.

## Search token quota

| Variable | Description |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | Tokens a workspace may spend on search per minute (default: `100000`). Charged once per query, before embedding, whether the request arrives at `GET /api/search` or through the `search_entities` MCP tool: one shared budget per workspace, not one per protocol. A query over budget gets HTTP `422` (`validation_failed`) instead of running. The default is high enough that ordinary use never reaches it; it exists to bound a runaway agent, not to ration normal traffic |

Search is metered in tokens rather than requests because that's what a query costs the embedding model; entity writes stay on request counts, since counting a large body is itself expensive.
The token count for a query comes from `EmbeddingProvider::count_tokens`, which the local provider overrides to the tokenizer's exact count and every other provider defaults to a byte-length estimate (`text.len() / 4`, rounded up).
That estimate is calibrated for English, where roughly 4 bytes make one token; Japanese text is roughly 3 bytes per character and tokenizes at roughly one token per character, so the estimate returns under half the token count a real tokenizer would report for the same Japanese query.
In other words, outside the local provider, a Japanese search query is charged against the budget at well under its real cost: `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` admits noticeably more Japanese-language search traffic per minute than English before this quota starts returning `422`.
Size the budget with that skew in mind if the deployment's search traffic is mostly Japanese and not running the local provider.

## What `config/production.yaml` requires

Three variables, and nothing else:

| Variable | Description |
|---|---|
| `DATABASE_URL` | The database connection URI. No default, since there is no safe one for a production database |
| `QUEUE_URL` | The queue backend's own URI, required on every `queue.kind` (see the section below) |
| `HOST` | The externally reachable hostname or address this deployment answers on |

Everything else this file reads either has a default or is opt-in.

**The mailer block is opt-in and renders only when `MAILER_HOST` is set to a non-empty value.** Nothing in this application sends mail, and `Config.mailer` is an `Option`, so an absent block is a supported state.
It is a template conditional rather than variables with defaults for a specific reason: loco-rs builds an `EmailSender` from this block whenever `smtp.enable` is true and fails the boot if that construction fails, so a block rendered against a placeholder host would fail where an unset variable would not.
Once you do set `MAILER_HOST`, `MAILER_USER` and `MAILER_PASSWORD` become required, which is the intended behaviour: opting in to mail means supplying credentials for it.

**There is no `auth:` block, deliberately.** That block configures loco-rs's own JWT support, and this application issues no JWTs: `POST /auth/login` returns a Yorishiro API key, and every authenticated path resolves it through `services::auth`, which never reads `Config.auth`.
`JWT_SECRET` is therefore not a variable this deployment reads at all.

`config/development.yaml` requires nothing: it boots against an empty environment, creating its own SQLite file.

## Logging

`logger.level` is what sets the level, and loco's own default filter is what selects the modules.
No `config/*.yaml` sets `override_filter`.

loco builds that default from a fixed module whitelist plus **one** entry for the application crate, taken from `Hooks::app_name()` (`loco-rs-1.1.0/src/logger.rs:192-210`).
One entry is enough because this workspace has one application crate: the enterprise edition compiles into it rather than being a crate of its own.

A second application crate would need naming in `override_filter`, or its events would be dropped by the filter with nothing in the symptom to point at why.

If you do set `override_filter` for your own reasons, two things follow that are easy to get wrong:

- **It replaces loco's whitelist rather than extending it.** The framework's own modules must be repeated in the value.
  Remove `loco_rs` and the queue, routing and `listening on` lines disappear; remove `sea_orm_migration` and migration progress does.
- **It replaces `logger.level` too**, which loco does not read at all when this is set, so `LOG_LEVEL` stops changing anything.

**`RUST_LOG` overrides all of this.** loco tries `EnvFilter::try_from_default_env()` before it looks at this configuration at all (`logger.rs:193`), so a deployment whose platform injects `RUST_LOG` gets whatever that variable says regardless of what is written here.
Measured: `RUST_LOG=loco_rs=info` produces zero lines from the application, with the server booting normally.
If `RUST_LOG` is set in your environment, it has to name `yorishiro` itself.

## Queue backend and tuning (`config/development.yaml`, `config/production.yaml`)

`queue.kind` is switchable at boot, since loco-rs ships three queue providers (Postgres, `SQLite`, Redis/Valkey, `QueueConfig`'s `#[serde(tag = "kind")]` variants) and each needs a different set of fields (Redis alone takes `queues`; Postgres/`SQLite` share the SQL-pool knobs but point at different URIs).
Both `development.yaml` and `production.yaml` template a whole alternative `queue:` block per `kind` (a Tera `<% if %>`/`<% elif %>`/`<% endif %>`) rather than templating individual fields inside one fixed shape.

| Variable | Description |
|---|---|
| `YORISHIRO_QUEUE_KIND` | `Sqlite` (default in `development.yaml`, matching that file's database default so an unconfigured start needs no Postgres), `Postgres`, or `Redis`. Booting with `Redis` needs the `worker_redis` Cargo feature compiled in (enabled in this workspace's `Cargo.toml`) or startup fails with "No queue provider feature was selected and compiled" |
| `QUEUE_URL` | The queue backend's own connection URI. In `development.yaml` it defaults to `DATABASE_URL` whenever the two backends agree, so an unconfigured start keeps its queue in the same SQLite file as its database and a PostgreSQL deployment keeps its queue in the same PostgreSQL instance. Where they disagree, which takes setting `YORISHIRO_QUEUE_KIND=Sqlite` against a PostgreSQL `DATABASE_URL`, the queue falls back to its own SQLite file rather than being handed a URI of the wrong scheme; `production.yaml` requires it explicitly with no default on every kind, matching that file's own no-silent-fallback convention |
| `YORISHIRO_QUEUE_WORKERS` | How many workers dequeue jobs in parallel (default: `2`). Postgres claims a row with `FOR UPDATE SKIP LOCKED`, so raising this genuinely adds parallelism on that backend; `SQLite`'s `BEGIN IMMEDIATE` serializes every dequeue regardless of this number |
| `YORISHIRO_QUEUE_REAPER_AGE_MINUTES` | Minutes a job may sit in `processing` before the reaper requeues it as `Queued` (default: `30`). Loco's own reaper is opt-in and off by default: without it, a job a worker died on while it was running (a crash, a forced kill) stays `processing` forever, since nothing else moves a job out of that state, `fail_job` only runs when `perform` itself returns an error. Set this above the longest a healthy job can legitimately take, or the reaper requeues work that is still genuinely in progress |

`development.yaml` enables the same reaper with fixed values (`num_workers: 2`, `age_minutes: 10`) rather than reading `YORISHIRO_QUEUE_WORKERS`/`YORISHIRO_QUEUE_REAPER_AGE_MINUTES`, since a local development environment has no reason to tune them per deployment; `production.yaml` reads both.
`config/test.yaml` has no `queue:` block at all (`docs`/`.claude/rules/testing.md` covers why), so none of this applies there.

`config/sqlite.yaml` (the manual-verification SQLite tier, `docs/sqlite.md`) also configures `queue: kind: Sqlite` with `workers.mode: BackgroundQueue`, the same as the other two environments.
loco-rs's `SQLite` queue provider (`bgworker::sqlt`) opens its own `sqlx::SqlitePool`, independent of the application's own `SQLite` connection, so it is a genuinely separate pool against the same or a different file, not routed through `db.rs`'s RLS-aware pool (`SQLite` has no RLS to be aware of).
Measured directly against a real file: a concurrent write from the queue pool while the application holds an open write transaction on the same file waits out `sqlx`'s own 5-second default `busy_timeout` and succeeds once that transaction releases the lock, rather than failing.
In this codebase specifically, the embedding-sync enqueue call only runs after the request's own write transaction has already committed, so this scenario does not arise from a single request; it would only matter for a genuinely concurrent second request racing the first's still-open transaction, and `content_entities::create` is one fast `INSERT`, well under the 5-second budget.

## Running workers on a separate process or host

`cargo loco start --worker[=tag1,tag2]` (or the equivalent `yorishiro` invocation, which shares loco-rs's own CLI) runs only the queue worker loop, no HTTP server, in the current process.
`--worker=worker-class:official` restricts that process to jobs carrying that tag (`WorkerClass::tag()`, `src/workers/embedding_sync.rs`).
A separate process, on a separate host, needs nothing beyond pointing its own config at the same `queue.uri`/`QUEUE_URL` and `database.uri`/`DATABASE_URL` the server uses: no additional networking layer, shared secret, or node-registration step.

**`--worker` with no value does not take every job.** Confirmed against `loco-rs` 1.1.0's own dequeue SQL (shared shape across the Postgres/`SQLite`/Redis queue providers): an empty tag list means "untagged jobs only", not "every job regardless of tag".
Every job this deployment enqueues always carries exactly one `worker-class:*` tag (`workers::embedding_sync::enqueue_for_class`), so it is never untagged, and a bare `--worker` process here dequeues none of these jobs rather than taking "the ones nothing else claimed".
A deployment that wants one process to cover every class must name every tag explicitly: `--worker=worker-class:tenant-private,worker-class:official,worker-class:shared`.
There is no wildcard/catch-all flag in `loco-rs` 1.1.0.

**A worker-only process still needs the server's full config, not just the queue connection.** `Hooks::after_context` (`src/app.rs`) runs unconditionally for every `StartMode` loco-rs has, including `--worker`-only: it builds the RLS-aware tenant pool and the migration-role identity pool against `DATABASE_URL` regardless of whether the process ever serves a request, and it fails boot outright if the embedding provider is misconfigured.
Every `WorkerClass` worker type's `perform` genuinely uses both: it reads `ctx.db` to re-fetch the entity and calls `resolve_embedding_provider`, which needs the same `YORISHIRO_EMBEDDING_*` variables (or a workspace's own assignment) the server needs.
A worker node configured with only a queue connection fails at boot rather than silently: the operator error this guards against is assuming "the worker only talks to the queue" and skipping the rest of the config.

**At least one process must stay subscribed to every `WorkerClass`'s tag, named explicitly.** If every running worker process is tag-restricted and no process names all three (`worker-class:tenant-private`, `worker-class:official`, `worker-class:shared`), whichever class none of them cover queues forever with nothing to dequeue it.
A deployment adding a dedicated `worker-class:official` node must keep (or add) at least one process still running with all three tags named (not bare `--worker`, see above) to cover `Shared` and any other class that node doesn't.

**What actually parallelizes across multiple worker processes/hosts depends on the queue backend**, the same distinction `YORISHIRO_QUEUE_WORKERS`'s own row above already draws for `num_workers` within one process.
Postgres's `pg_loco_queue` dequeue uses `FOR UPDATE SKIP LOCKED`, so multiple processes (on one host or several) genuinely dequeue different jobs concurrently.
`SQLite`'s `sqlt_loco_queue` dequeue uses `BEGIN IMMEDIATE`, which takes the file's one write lock, so a second process pointed at the same `SQLite` file serializes behind the first.
Running more than one worker process against a `SQLite`-backed queue therefore buys resilience (a second process to pick up work if the first dies) but not throughput.

### A workspace's own worker-class assignment (enterprise edition)

`PUT /api/workspace/worker-class` pins one workspace's embedding-sync jobs to `tenant_private` or `official` compute instead of the shared pool every workspace uses by default.
Not part of the base edition: which compute a tenant's jobs run on is the same enterprise-edition decision that already assigns LLM/embedding credentials per workspace (`PUT /api/workspace/llm-key`, `PUT /api/workspace/embedding-key`).

| Field | Description |
|---|---|
| `worker_class` | One of `tenant_private`, `official`, `shared` |

A workspace with nothing configured here keeps its jobs `shared`, so a deployment that assigns nothing is unaffected by this endpoint.
`DELETE /api/workspace/worker-class` returns a workspace to `shared`.
No caching: an assignment made through this endpoint takes effect on the very next job enqueued for that workspace, not after some delay or a restart.
Assigning a workspace to `tenant_private`/`official` has no effect on its own until a worker process actually subscribes to that tag ("Running workers on a separate process or host" above).
A workspace can be assigned a class with no node running it yet, and its jobs simply queue until one does.
