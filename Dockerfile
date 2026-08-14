# Multi-stage build producing a self-contained runtime image. Used both for production
# distribution and as the `app` service in docker-compose.yml; day-to-day development
# (test/fmt/clippy) instead runs through the `dev` service, see .devcontainer/Dockerfile.
#
#   docker build -t yorishiro .
#   docker run --rm -e DATABASE_URL=... -e YORISHIRO_EMBEDDING_PROVIDER=... yorishiro
#
# Note: the `ort` crate fetches an onnxruntime binary at build time, so the build needs
# network access (for air-gapped builds, see ORT_LIB_LOCATION in the README).
# The SPA is embedded into the binary from `ee/web/dist`, which is a build output and is not
# committed -- so it is built first, in its own stage, and copied into the cargo build.
FROM node:24-slim AS web
RUN corepack enable
WORKDIR /web
COPY ee/web/package.json ee/web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY ee/web/ ./
RUN pnpm run build

FROM rust:1.97-slim AS builder

# curl is required by utoipa-swagger-ui's build.rs (fetches the Swagger UI assets).
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    g++ \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
# Overwrites whatever `dist` the build context carried (normally just `.gitkeep`) with the one
# the `web` stage just produced, so the embed is always this build's own output.
COPY --from=web /web/dist ee/web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/root/.cache/ort.pyke.io \
    cargo build --release -p yorishiro-hosted \
    && cp target/release/yorishiro-hosted-server /usr/local/bin/yorishiro-hosted-server

# onnxruntime is statically linked, so the only shared library needed at runtime is
# libstdc++6 (plus ca-certificates for the OpenAI-compatible provider's TLS, and curl for
# the HEALTHCHECK below). Keep the base (debian trixie, matching builder's rust:1.97-slim)
# on the same glibc as the builder.
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home yorishiro

COPY --from=builder /usr/local/bin/yorishiro-hosted-server /usr/local/bin/yorishiro-hosted-server

# The docs say `yorishiro-hosted-server admin ...`, and that has to work inside the image. The
# old name stays as a symlink: every existing `docker run ... yorishiro-server admin` keeps
# working rather than failing with "not found" on an upgrade.
RUN ln -s /usr/local/bin/yorishiro-hosted-server /usr/local/bin/yorishiro-server

# Relative paths in embedding provider settings (e.g. YORISHIRO_ONNX_MODEL_PATH=models/model.onnx)
# resolve against this directory, so a model directory can be bind-mounted here without
# also needing an absolute-path override.
WORKDIR /app
RUN chown -R yorishiro:yorishiro /app

USER yorishiro
# The one binary defaults to 8081 (`YORISHIRO_BIND`). Set it here so the image keeps
# serving on 8080, which every existing compose file, healthcheck and deployment expects.
ENV YORISHIRO_BIND=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
    CMD curl -sf http://localhost:8080/up || exit 1
ENTRYPOINT ["yorishiro-hosted-server"]
