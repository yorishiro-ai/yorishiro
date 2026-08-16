# Embedding Providers

**English** | [日本語](ja/embedding-providers.md)

Embedding generation for `x-embed` fields is switched with `YORISHIRO_EMBEDDING_PROVIDER`.
The dimension count is configurable via `YORISHIRO_EMBEDDING_DIMENSIONS` (default 1024) and must match the model's output.
Embeddings are generated asynchronously in the background after an entity is written, so write API latency is unaffected.

## `local` — Local ONNX model (default)

Requires no external service or API key — just the model files below — so it's the default and what a self-hosted deployment normally wants.
Requires a BERT-family ONNX export at `YORISHIRO_ONNX_MODEL_PATH`/`YORISHIRO_ONNX_TOKENIZER_PATH`, which already default to `models/model.onnx`/`models/tokenizer.json`.
The default model (multilingual-e5-large) outputs 1024-dimensional vectors and covers 100+ languages, so Japanese and English text describing the same thing land near each other:

```console
$ mkdir -p models
$ curl -L -o models/model.onnx \
    https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/onnx/model_quantized.onnx
$ curl -L -o models/tokenizer.json \
    https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/tokenizer.json
```

Placing the two files at those default paths is enough -- no environment variables are required at all.
Note: "requires no external service" applies at runtime only.

**At build time**, the `ort` crate downloads a prebuilt onnxruntime binary (from cdn.pyke.io).
If your build environment is also air-gapped, provide a pre-placed onnxruntime and point the build at it with the `ORT_LIB_LOCATION` environment variable.

## `openai` — OpenAI-compatible API

Uses an `/v1/embeddings`-compatible endpoint such as Ollama, LM Studio, or OpenAI.
Set `YORISHIRO_EMBEDDING_PROVIDER=openai` explicitly to opt into this instead of the local ONNX default:

```dotenv
YORISHIRO_EMBEDDING_PROVIDER=openai
YORISHIRO_EMBEDDING_BASE_URL=http://localhost:11434/v1
YORISHIRO_EMBEDDING_MODEL=nomic-embed-text
```

### When the provider is busy

A `429` or `503` from the provider is a request to come back, not a rejection.
The embedding sync waits the `Retry-After` the provider asked for, capped at 60 seconds and defaulting to a short wait when the header is absent, and retries up to three times before giving up and leaving the entity to `admin resync-embeddings`.

Anything else (a `400`, say) is a request the provider will never accept, and is not retried: spending the attempts on it would not help, and the error says so in the log.

This matters because embedding happens after the response.
An entity whose embedding is lost to a rate limit is written and durable, but absent from semantic search until a resync. The retry is what keeps a busy minute at the provider from quietly costing you that.

### When the provider cannot be reached

A provider that never answers at all (the process is down, the port is wrong, DNS does not resolve) is reported as `502 Bad Gateway`, naming the configured base URL:

```json
{
  "error": {
    "message": "the embedding provider at http://localhost:11434/v1 could not be reached: error sending request",
    "hint": "check that the provider is running and that YORISHIRO_EMBEDDING_BASE_URL points at it"
  }
}
```

This is separate from the `503` above on purpose.
A `503` means the provider answered and asked for a wait, so retrying on its schedule is the right response.
A `502` means there was nothing there to answer, which waiting does not fix: it is a configuration error or an outage, and the response names the endpoint so you can tell those apart from a defect in the query itself.

`GET /api/search` is where this usually surfaces, since it embeds the query before it can search.
