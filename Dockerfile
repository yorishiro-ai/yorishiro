# syntax=docker/dockerfile:1.29.0

ARG DEBIAN_IMAGE=debian:trixie-slim

# Package binaries use the oldest supported glibc, independently of the container runtime.
FROM almalinux:10 AS package-builder
RUN dnf install -y gcc gcc-c++ openssl-devel pkgconf-pkg-config git binutils \
    && dnf clean all
ENV PATH=/usr/local/cargo/bin:$PATH
ENV RUSTUP_HOME=/usr/local/rustup
# CI mounts its toolchain; bfd is available here without the host's mold or sccache.
ENV RUSTC_WRAPPER=""
ENV RUSTFLAGS="-C link-arg=-fuse-ld=bfd"

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
COPY config/production.yaml config/production.yaml
COPY benches/ benches/
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
# Root ownership and the sticky bit protect root-owned state files from the service account.
RUN mkdir -p /var/lib/yorishiro \
    && chown root:yorishiro /var/lib/yorishiro \
    && chmod 1770 /var/lib/yorishiro

WORKDIR /app
RUN install -d -o yorishiro -g yorishiro /var/lib/yorishiro/state /app/models /app/config

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
