# Environment Variable Reference

**English** | [日本語](ja/configuration.md)

The full list of variables, with comments, lives in [`.env.example`](../.env.example).
Variables are passed to the server **as process environment variables** -- there is no mechanism that automatically reads a `.env` file.
Set them via `environment:` in docker compose, `docker compose exec -e`, `Environment=` in systemd, or similar.

## The `YSR_` prefix is deprecated

Every variable is named `YORISHIRO_*`.
The old `YSR_*` names are still accepted: the server copies each onto its replacement at startup and prints a warning naming both.
The same applies to the `YORISHIRO_HOSTED_*` names, which distinguished a second binary that no longer exists — `YSR_WEB_DIR` and `YORISHIRO_HOSTED_WEB_DIR` both become `YORISHIRO_WEB_DIR`.
Setting the new name alongside an old one uses the new value.

The rename happens before `config.yml` is read, so an exported old name still beats a value in the file, exactly as the new name would.

## config.yml

Every setting below can also go in a `config.yml` file instead.
See [`config.example.yml`](../config.example.yml) for the full key list (nested under `embedding:`, `logging:`, and `auth_rate_limit:` for those groups).
By default the server looks for `config.yml` in its working directory; set `YORISHIRO_CONFIG_PATH` to point elsewhere.

A missing file, or a missing key within it, is not an error -- that setting just falls back to its usual default.
**A set environment variable always wins over the equivalent `config.yml` key.**
An *unknown* key (e.g. a typo) is rejected: the server fails to start rather than silently ignoring it.

This makes `config.yml` convenient as the base configuration for a deployment, with environment variables reserved for one-off overrides (e.g. a Docker `-e` flag for a single run) rather than the only way to configure anything.

## Core

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string (required) |
| `YORISHIRO_CONFIG_PATH` | Path to the `config.yml` file described below (default: `config.yml` in the working directory) |
| `YORISHIRO_BIND` | Listen address (default: `0.0.0.0:8080`) |
| `YORISHIRO_CORS_ORIGINS` | Comma-separated list of allowed origins for browser access (e.g. so a browser-based dashboard on a different origin can call `/auth/login`/`/api/members`). Cross-origin reads are disabled if unset. In debug builds only, leaving this unset also auto-allows any `http://localhost:*`/`http://127.0.0.1:*` origin (for browser-based dev tools like the MCP Inspector) -- release builds never do this |
| `YORISHIRO_MAX_TENANTS` | Deployment-wide cap on tenants `admin create-tenant` may create. Defaults to `1` (single-tenant). Set `0` for unlimited, or a higher number for that many. `POST /auth/signup` never creates a tenant (it just redeems an invite), so it's unaffected. Also gates the first-run setup wizard (see [setup.md](setup.md#first-run-setup)), enabled only when the cap isn't `0` |
| `YORISHIRO_WEB_DIR` | The web UI is compiled into the binary from `ee/web/dist` and served at `/` by default. Set this to serve it from a real directory on disk instead, read fresh on every request, to iterate on the UI without rebuilding |
| `YORISHIRO_AUTH_RATE_LIMIT_MAX` / `YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS` | Per-client-IP rate limit on `/auth/signup`, `/auth/login`, and `/setup` — the endpoints reachable without a bearer token, and therefore the only ones an unauthenticated caller can brute-force. Defaults: 10 requests per 60 seconds |
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | Tokens a workspace may spend on search per minute (default: `100000`). Search is metered in tokens rather than requests because that is what it costs the embedding model; writes stay on request counts, since counting a large body costs more than the write. A query over budget still runs once and leaves the window spent, rather than being permanently impossible |
| `YORISHIRO_SNAPSHOT_RETENTION_DAYS` | How many days a batch migration stays undoable (default: `30`; `0` or less keeps every before-image forever). A migration writes one image per entity it touches, and only an undo takes them away, so an unbounded workspace that migrates repeatedly ends up holding more images than entities. The sweep runs at the start of the next migration in that workspace rather than on a timer. Undoing a job past the window answers `404`, the same as a job that never ran. A value that is not a 32-bit integer falls back to the default rather than being clamped — six million years of retention is a typo, and honouring the nearest legal value would hide it |
| `RUST_LOG` | Log level (e.g. `info`) |

## Database load guard

Drops the deployment to read-only while the database is under sustained load, and restores it when the load falls away.
Off unless a threshold is set: dropping a deployment to read-only uninvited is a large thing to do on a default, and the right number depends on `max_connections`, which the server does not choose.

| Variable | Description |
|---|---|
| `YORISHIRO_DB_LOAD_THRESHOLD` | Active connections above which the deployment goes read-only. Unset or `0` disables the guard entirely |
| `YORISHIRO_DB_LOAD_SUSTAIN_SECS` | How long the threshold must be exceeded before switching (default: `30`). Stops a momentary spike from tripping it |
| `YORISHIRO_DB_LOAD_POLL_SECS` | How often the connection count is sampled (default: `5`) |

## Request correlation

Every response carries an `x-request-id` header -- a UUID the server generates if the request didn't already have one, otherwise the caller's own value is echoed back unchanged.
The same value tags the tracing span for that request, so any `warn`/`error` line logged while handling it (an authentication rejection, a rate-limit hit, an internal error) carries the same `request_id` field as the access log line for that request.
Useful for tying a specific failed request to its server-side log lines when following up on an incident report.

Rejected requests (bad/missing API key, insufficient scope, rate limit exceeded) are logged at `warn` with the caller's IP and the request path, but never the presented credential -- previously these surfaced only as an anonymous 401/403/429 in the access log.

## Logging

Every log line, including the HTTP access log (method, path, status, latency), is a JSON object.
`YORISHIRO_LOG_TARGET` selects where those lines go:

| Variable | Description |
|---|---|
| `YORISHIRO_LOG_TARGET` | `stdout` (default, for a container runtime's log driver), `single` (one file, never rotated), `daily` (one file per day), or `syslog` (Unix only -- rejected at startup on other platforms) |

### When `YORISHIRO_LOG_TARGET=single` or `daily`

| Variable | Description |
|---|---|
| `YORISHIRO_LOG_DIR` | Directory the log file is written under (default: `.`). The file is named `yorishiro.log`, with the date appended for `daily` (e.g. `yorishiro.log.2026-07-13`) |

### When `YORISHIRO_LOG_TARGET=syslog`

| Variable | Description |
|---|---|
| `YORISHIRO_SYSLOG_SOCKET` | Unix domain socket to send RFC 3164-framed messages to (default: `/dev/log`). Linux/Unix only |

## Embedding provider

| Variable | Description |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` (default) or `openai` |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | Dimensionality of the embedding vectors (default: `1024`, the width of the default model). Must match the model's output dimension. A workspace is stamped with this value when it is created, and a later write produced by a different model is refused — see below |

### When `YORISHIRO_EMBEDDING_PROVIDER=local` (ONNX export, the default)

| Variable | Description |
|---|---|
| `YORISHIRO_ONNX_MODEL_PATH` | Path to the ONNX model (default: `models/model.onnx`) |
| `YORISHIRO_ONNX_TOKENIZER_PATH` | Path to the tokenizer (default: `models/tokenizer.json`) |
| `YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH` | Maximum sequence length (default: `512`) |
| `YORISHIRO_ONNX_POOLING` | How token embeddings are reduced to one vector: `mean` (default) or `last_token`. This is a property of the model, not a preference — sentence-transformers exports (bge-small, multilingual-e5, all-mpnet) want `mean`, the Qwen3-Embedding family wants `last_token`. Reading a model with the wrong one raises no error; the search results just get worse, so an unrecognized value fails startup rather than falling back |
| `YORISHIRO_ONNX_QUERY_INSTRUCTION` | Instruction prefixed to search queries only. Qwen3-Embedding expects `Instruct: {task}\nQuery:{text}`; stored documents never get it. Unset or empty disables it (the default). Leave unset for symmetric models |

### Changing the embedding model

A workspace records the model and dimension count it was created under.
A write whose vector is a different width is refused with `422`, naming both numbers.

Without that check the write would succeed — the column is dimensionless — and the workspace's next search would fail with `different vector dimensions 384 and 1024`, naming neither the entity nor the write that caused it.

To move a workspace to another model, point the deployment at it and re-embed:

```console
$ yorishiro-server admin resync-embeddings --workspace <id>
```

Workspaces created before this stamp existed carry none, and accept whatever the deployment produces — which is what they have always done.


### When `YORISHIRO_EMBEDDING_PROVIDER=openai` (e.g. Ollama, LM Studio, OpenAI)

| Variable | Description |
|---|---|
| `YORISHIRO_EMBEDDING_BASE_URL` | Base URL of the `/v1/embeddings`-compatible endpoint (required) |
| `YORISHIRO_EMBEDDING_MODEL` | Model name (required) |
| `YORISHIRO_EMBEDDING_API_KEY` | API key, if required by the endpoint |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | Whether to include a `dimensions` parameter in the request body. Defaults to `true` when unset. Once set, only the exact lowercase string `true` keeps it enabled -- every other value, including `false`, `False`, `FALSE`, and `0`, disables it |

See [docs/embedding-providers.md](embedding-providers.md) for a worked example, e.g. `https://huggingface.co/Xenova/multilingual-e5-large` (`onnx/model_quantized.onnx` and `tokenizer.json`).
