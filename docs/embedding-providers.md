# Embedding Providers

**English** | [日本語](ja/embedding-providers.md)

Embedding generation for `x-embed` fields is switched with `YSR_EMBEDDING_PROVIDER`.
The dimension count is configurable via `YSR_EMBEDDING_DIMENSIONS` (default 1024) and must match the model's output.
Embeddings are generated asynchronously in the background after an entity is written, so write API latency is unaffected.

## `local` — Local ONNX model (default)

Requires no external service or API key — just the model files below — so it's the default and what a self-hosted deployment normally wants.
Requires a BERT-family ONNX export at `YSR_ONNX_MODEL_PATH`/`YSR_ONNX_TOKENIZER_PATH`, which already default to `models/model.onnx`/`models/tokenizer.json`.
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
Set `YSR_EMBEDDING_PROVIDER=openai` explicitly to opt into this instead of the local ONNX default:

```dotenv
YSR_EMBEDDING_PROVIDER=openai
YSR_EMBEDDING_BASE_URL=http://localhost:11434/v1
YSR_EMBEDDING_MODEL=nomic-embed-text
```

### When the provider is busy

A `429` or `503` from the provider is a request to come back, not a rejection.
The embedding sync waits the `Retry-After` the provider asked for — capped at 60 seconds, and defaulting to a short wait when the header is absent — and retries up to three times before giving up and leaving the entity to `admin resync-embeddings`.

Anything else (a `400`, say) is a request the provider will never accept, and is not retried: spending the attempts on it would not help, and the error says so in the log.

This matters because embedding happens after the response.
An entity whose embedding is lost to a rate limit is written and durable, but absent from semantic search until a resync — the retry is what keeps a busy minute at the provider from quietly costing you that.
