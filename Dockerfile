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
ARG EDITION=ee

RUN apt-get update && apt-get install -y \
    g++ \
    curl \
    mold \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
# The edition is selected at build time with EDITION=ce or EDITION=ee. CE does not compile the
# enterprise module; EE includes it and retains its runtime licence gate.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    RUSTC_WRAPPER= cargo install sccache --locked --version 0.10.0 \
    && export RUSTC_WRAPPER=/usr/local/cargo/bin/sccache \
    && (case "$EDITION" in \
      ce) cargo build --locked --no-default-features --release --bin yorishiro ;; \
      ee) cargo build --locked --features enterprise --release --bin yorishiro ;; \
      *) echo "unknown EDITION=$EDITION" >&2; exit 1 ;; \
    esac) \
    && cp target/release/yorishiro /usr/local/bin/yorishiro

# The binary uses the system C++ runtime for candle/tokenizers support, so the runtime image
# installs the same libgcc/libstdc++ dependencies declared by the native packages.
# ca-certificates is for the OpenAI-compatible provider's TLS, curl for the HEALTHCHECK.
# The base stays on the same glibc as the builder (debian trixie, matching rust:1.97-slim).
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libgcc-s1 \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/yorishiro yorishiro

COPY --from=builder /usr/local/bin/yorishiro /usr/local/bin/yorishiro
# Keep Loco's environment-based configuration layout in the runtime image.
COPY config/production.yaml /app/config/production.yaml

# Relative paths in embedding provider settings (YORISHIRO_LOCAL_MODEL_PATH defaults to
# `models/model.safetensors`) resolve against this directory, so a model directory can be
# bind-mounted here without also needing an absolute-path override. Without a mount the provider
# fetches the model on first use instead, into $HOME/.cache/yorishiro/models.
WORKDIR /app
# The production configuration defaults SQLite state to /var/lib/yorishiro.
# Create it in the image so an unconfigured non-root container can boot and persist data.
RUN chown -R yorishiro:yorishiro /app \
    && install -d -o yorishiro -g yorishiro /var/lib/yorishiro \
    && test -w /var/lib/yorishiro

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
ENV LOCO_ENV=production
ENV LOCO_CONFIG_FOLDER=/app/config
USER yorishiro
# 5150 is the server's own default port (`config/production.yaml`'s `server.port`), which is what the
# compose file and the healthcheck below expect.
EXPOSE 5150
# `/_ping` rather than `/_health`: both come from loco's default routes, and `_ping` answers
# without touching the database, so this reports on the process rather than on its dependencies.
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
    CMD curl -sf http://localhost:5150/_ping || exit 1
ENTRYPOINT ["yorishiro"]
CMD ["start"]
