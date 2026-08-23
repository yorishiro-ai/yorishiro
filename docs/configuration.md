# Configuration Reference

**English** | [日本語](ja/configuration.md)

This is not a full settings reference: it covers the embedding provider and the per-workspace search token quota only, the two areas the Loco rebuild's most recent slice touched.
Every variable listed here is read directly from the environment; there is no `config.yml`-style file for these settings on this branch.

## Embedding provider

`build_embedding_provider` (`src/services/embedding/mod.rs`) selects and configures the provider used for both writing embeddings (`sync_embedding`) and search (`GET /api/search`, the `search_entities` MCP tool).

| Variable | Description |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` selects the local ONNX provider (see below). Anything else, or unset, selects the OpenAI-compatible provider |
| `YORISHIRO_EMBEDDING_BASE_URL` | OpenAI-compatible provider only. Base URL of an OpenAI-compatible embeddings endpoint (LM Studio, Ollama, vLLM, or real OpenAI), e.g. `http://10.0.3.200:1234/v1`. Required together with `YORISHIRO_EMBEDDING_MODEL`: if either is unset, boot proceeds with no embedding backend configured rather than failing, and every embed call fails at request time with `ProviderUnreachable` instead |
| `YORISHIRO_EMBEDDING_MODEL` | OpenAI-compatible provider only. Model name sent in the `model` field of the embeddings request. Also stamped onto a workspace at creation time as the model it was embedded with; unset, a workspace is stamped `unconfigured` |
| `YORISHIRO_EMBEDDING_API_KEY` | OpenAI-compatible provider only. Bearer token sent to `YORISHIRO_EMBEDDING_BASE_URL`. Empty by default, which is correct for a local server (LM Studio, Ollama) that doesn't check one |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | Expected vector width (default: `768`). Every vector in a deployment must share this width; the local ONNX provider verifies it at startup with a probe inference, the OpenAI-compatible provider verifies it per response |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | OpenAI-compatible provider only. `true` includes a `dimensions` field in the embeddings request. Default `false`, since some OpenAI-compatible implementations (vLLM, Ollama, LM Studio) reject a `dimensions` field they don't recognise |

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
