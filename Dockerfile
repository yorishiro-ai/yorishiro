# syntax=docker/dockerfile:1.29.0

ARG DEBIAN_IMAGE=debian:trixie-slim
FROM ${DEBIAN_IMAGE} AS builder

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
RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --default-toolchain none \
    && rm /tmp/rustup-init.sh \
    && rustup toolchain install --no-self-update
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml .cargo/config.toml
COPY benches/sqlite_ann.rs benches/sqlite_ann.rs
COPY src/ src/
COPY ee/ ee/
COPY migration/ migration/
COPY patches/loco-rs/ patches/loco-rs/
RUN python3 patches/loco-rs/verify.py --prepare
ARG EDITION=ee
ARG BUILD_SHA=dev
ENV BUILD_SHA=${BUILD_SHA}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target,sharing=locked \
    export RUSTC_WRAPPER= \
    && (case "$EDITION" in \
      ce) cargo build --locked --no-default-features --features community --release --bin yorishiro ;; \
      ee) cargo build --locked --features enterprise --release --bin yorishiro ;; \
      *) echo "unknown EDITION=$EDITION" >&2; exit 1 ;; \
    esac) \
    && cp target/release/yorishiro /usr/local/bin/yorishiro

# Keep the builder and runtime on the same glibc.
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
COPY config/production.yaml /app/config/production.yaml

WORKDIR /app
RUN install -d -o yorishiro -g yorishiro /var/lib/yorishiro/state /app/models

ENV HOME=/home/yorishiro
ENV LOCO_ENV=production
ENV LOCO_CONFIG_FOLDER=/app/config
USER yorishiro
EXPOSE 5150
# Probe process liveness without touching the database.
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s \
    CMD curl -sf http://localhost:5150/_ping || exit 1
ENTRYPOINT ["yorishiro"]
CMD ["start"]
