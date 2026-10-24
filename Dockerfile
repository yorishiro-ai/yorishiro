# Multi-stage build producing a self-contained runtime image, used both for distribution and as
# the `app` service in compose.yml.
#
#   docker compose up --build
#   docker run --rm -e DATABASE_URL=... -e QUEUE_URL=... -e HOST=... yorishiro
#
# The default command serves HTTP only. Queued jobs run in a second container from this same
# image, started with the target it serves (see `docker-compose.yml`'s `worker`), and with the
# HEALTHCHECK below disabled because a worker answers on no port:
#
#   docker run --rm yorishiro start --worker=worker-target:shared
#
# Local embedding inference uses Candle and safetensors without a separate native runtime.

ARG DEBIAN_IMAGE=debian:trixie-slim
FROM ${DEBIAN_IMAGE} AS builder
ARG EDITION=ee

RUN apt-get update && apt-get install -y \
    g++ \
    curl \
    mold \
    pkg-config \
    python3 \
    patch \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH
COPY rust-toolchain.toml .
RUN channel=$(python3 -c 'import tomllib; print(tomllib.load(open("rust-toolchain.toml", "rb"))["toolchain"]["channel"])') \
    && curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain "$channel" \
    && rm /tmp/rustup-init.sh
COPY . .
RUN python3 patches/loco-rs/verify.py --prepare
# The edition is selected at build time with EDITION=ce or EDITION=ee. CE does not compile the
# enterprise module; EE includes it and retains its runtime licence gate.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target,sharing=locked \
    export RUSTC_WRAPPER= \
    && (case "$EDITION" in \
      ce) cargo build --locked --no-default-features --features community --release --bin yorishiro ;; \
      ee) cargo build --locked --features enterprise --release --bin yorishiro ;; \
      *) echo "unknown EDITION=$EDITION" >&2; exit 1 ;; \
    esac) \
    && cp target/release/yorishiro /usr/local/bin/yorishiro

# The binary uses the system C++ runtime for candle/tokenizers support, so the runtime image
# installs the same libgcc/libstdc++ dependencies declared by the native packages.
# ca-certificates is for the OpenAI-compatible provider's TLS, curl for the HEALTHCHECK.
# The builder and runtime share the same base and glibc.
FROM ${DEBIAN_IMAGE}

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    libgcc-s1 \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/yorishiro yorishiro

COPY --from=builder /usr/local/bin/yorishiro /usr/local/bin/yorishiro
# The state directory is root-owned and sticky, so the yorishiro user can create and update its own
# files in it but cannot replace or rename anything root put there.
RUN mkdir -p /var/lib/yorishiro \
    && chown root:yorishiro /var/lib/yorishiro \
    && chmod 1770 /var/lib/yorishiro
# The image owns the canonical runtime configuration and loads it through LOCO_CONFIG_FOLDER.
COPY config/production.yaml /app/config/production.yaml

# Mount pinned artifacts under /app/models/<short_id> or fetch them into the home cache on first use.
WORKDIR /app
# The production configuration defaults SQLite state to /var/lib/yorishiro.
# The directory is already created with root:yorishiro 1770 above.
RUN chown -R yorishiro:yorishiro /app \
    && mkdir -p /var/lib/yorishiro/state /home/yorishiro/.cache/yorishiro \
    && chown -R yorishiro:yorishiro /var/lib/yorishiro/state /home/yorishiro/.cache/yorishiro

# Automatic model fetching requires a writable home cache.
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
