# Multi-stage build producing a self-contained runtime image, used both for distribution and as
# the `app` service in compose.yml.
#
#   docker build -t yorishiro .
#   docker run --rm -e DATABASE_URL=... -e QUEUE_URL=... -e HOST=... yorishiro
#
# The embedding provider (`candle-core`/`candle-nn`/`candle-transformers`) needs no prebuilt
# runtime binary and pulls no dynamic TLS library into the link, unlike the ONNX-based provider
# this Dockerfile used to build.
#
# There is no web-asset stage. An earlier version of this file built `ee/web` with pnpm and
# copied the result into the cargo build so the SPA could be embedded. That directory does not
# exist on this branch: the frontend is not part of the rebuild yet. When it returns, its stage
# comes back with it, in the same change that adds the directory.
FROM rust:1.97-slim AS builder

RUN apt-get update && apt-get install -y \
    g++ \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
# One package, one binary. Both editions are in it: `ee/` compiles into this crate as a module,
# and which features serve is decided at runtime by the licence layer, so there is no
# edition-specific build to select here.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin yorishiro \
    && cp target/release/yorishiro /usr/local/bin/yorishiro

# No libstdc++6 here: `readelf -V` on the built binary shows no GLIBCXX version requirement and
# no libstdc++.so in NEEDED at all, unlike when this Dockerfile built the ort-based provider.
# `onig_sys` (tokenizers' oniguruma dependency) and candle's C++ kernels are both statically
# linked into the binary rather than dynamically linked at runtime.
# ca-certificates is for the OpenAI-compatible provider's TLS, curl for the HEALTHCHECK.
# The base stays on the same glibc as the builder (debian trixie, matching rust:1.97-slim).
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/yorishiro yorishiro

COPY --from=builder /usr/local/bin/yorishiro /usr/local/bin/yorishiro
# The application reads `config/{LOCO_ENV}.yaml` relative to its working directory, so the
# config directory ships in the image rather than being mounted: the values inside it come from
# the environment (`get_env(...)`), which is what a deployment actually sets.
COPY config/ /app/config/

# Relative paths in embedding provider settings (YORISHIRO_LOCAL_MODEL_PATH defaults to
# `models/model.safetensors`) resolve against this directory, so a model directory can be
# bind-mounted here without also needing an absolute-path override. Without a mount the provider
# fetches the model on first use instead, into $HOME/.cache/yorishiro/models.
WORKDIR /app
RUN chown -R yorishiro:yorishiro /app

# The account has a real home, and `HOME` is set for it, because the model fetch needs somewhere
# to write. `services::embedding::model_fetch::cache_dir` reads `HOME` and gives up when it is
# unset or empty, which makes the provider degrade to erroring on every call rather than
# fetching. `compose.yml` selects the local provider and deliberately leaves the local model
# paths unset so that fetch is what runs, so an image whose user had no home would take exactly
# that degraded path on a machine with no model mounted.
#
# `--no-create-home` was what this used, and `src/services/embedding/mod.rs` cites a
# `--no-create-home` container as the reason the no-`HOME` branch exists at all. It was
# describing this image.
ENV HOME=/home/yorishiro
USER yorishiro
# `production` takes no silent default for a secret or an address: it fails at startup when
# DATABASE_URL, QUEUE_URL or HOST is unset rather than falling back to a development value.
# The CLI's own default is `development`, so this is required, not decorative.
ENV LOCO_ENV=production
# 5150 is the server's own default port (`config/production.yaml`'s `PORT`), which is what the
# compose file and the healthcheck below expect.
EXPOSE 5150
# `/_ping` rather than `/_health`: both come from loco's default routes, and `_ping` answers
# without touching the database, so this reports on the process rather than on its dependencies.
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
    CMD curl -sf http://localhost:5150/_ping || exit 1
ENTRYPOINT ["yorishiro"]
CMD ["start"]
