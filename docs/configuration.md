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

## Queue tuning (`config/production.yaml`)

`config/production.yaml`'s `queue:` block accepts two settings beyond what `development.yaml` hardcodes, both of Loco's own `queue` config schema, not something this codebase adds.

| Variable | Description |
|---|---|
| `YORISHIRO_QUEUE_WORKERS` | How many workers dequeue jobs in parallel (default: `2`). Postgres claims a row with `FOR UPDATE SKIP LOCKED`, so raising this genuinely adds parallelism on this deployment's Postgres-backed queue |
| `YORISHIRO_QUEUE_REAPER_AGE_MINUTES` | Minutes a job may sit in `processing` before the reaper requeues it as `Queued` (default: `30`). Loco's own reaper is opt-in and off by default: without it, a job a worker died on while it was running (a crash, a forced kill) stays `processing` forever, since nothing else moves a job out of that state, `fail_job` only runs when `perform` itself returns an error. Set this above the longest a healthy job can legitimately take, or the reaper requeues work that is still genuinely in progress |

`development.yaml` enables the same reaper with fixed values (`num_workers: 2`, `age_minutes: 10`) rather than reading these variables, since a local development environment has no reason to tune them per deployment.
`config/test.yaml` has no `queue:` block at all (`docs`/`.claude/rules/testing.md` covers why), so neither setting applies there.
