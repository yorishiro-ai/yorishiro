# Multi-stage build producing a self-contained runtime image, used both for distribution and as
# the `app` service in compose.yml.
#
#   docker build -t yorishiro .
#   docker run --rm -e DATABASE_URL=... -e QUEUE_URL=... -e HOST=... yorishiro
#
# The `ort` crate fetches an onnxruntime binary at build time, so the build needs network access;
# point ORT_LIB_LOCATION at a pre-provisioned onnxruntime for an air-gapped build.
#
# There is no web-asset stage. An earlier version of this file built `ee/web` with pnpm and
# copied the result into the cargo build so the SPA could be embedded. That directory does not
# exist on this branch: the frontend is not part of the rebuild yet. When it returns, its stage
# comes back with it, in the same change that adds the directory.
FROM rust:1.97-slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
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
    --mount=type=cache,target=/root/.cache/ort.pyke.io \
    cargo build --release --bin yorishiro \
    && cp target/release/yorishiro /usr/local/bin/yorishiro

# onnxruntime is statically linked, so the only shared library needed at runtime is libstdc++6,
# plus ca-certificates for the OpenAI-compatible provider's TLS and curl for the HEALTHCHECK.
# The base stays on the same glibc as the builder (debian trixie, matching rust:1.97-slim).
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home yorishiro

COPY --from=builder /usr/local/bin/yorishiro /usr/local/bin/yorishiro
# The application reads `config/{LOCO_ENV}.yaml` relative to its working directory, so the
# config directory ships in the image rather than being mounted: the values inside it come from
# the environment (`get_env(...)`), which is what a deployment actually sets.
COPY config/ /app/config/

# Relative paths in embedding provider settings (YORISHIRO_ONNX_MODEL_PATH defaults to
# `models/model.onnx`) resolve against this directory, so a model directory can be bind-mounted
# here without also needing an absolute-path override. Without a mount the provider fetches the
# model on first use instead, into $HOME/.cache/yorishiro/models.
WORKDIR /app
RUN chown -R yorishiro:yorishiro /app

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
