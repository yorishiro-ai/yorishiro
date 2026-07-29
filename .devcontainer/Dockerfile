FROM rust:1.97-slim

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    curl \
    g++ \
    && rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml pins channel + components ("stable" here, but toolchain files can name
# an exact version too); resolving it now, at build time, bakes that toolchain into the image
# so that mounting the repo over /workspace at runtime doesn't trigger a network fetch.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN cd /tmp && rustup show

RUN cargo install sqlx-cli --no-default-features --features postgres

ARG UID=1000
ARG GID=1000
RUN groupadd -g $GID dev && useradd -u $UID -g $GID -m dev \
    && mkdir -p /usr/local/cargo/registry && chown -R dev:dev /usr/local/cargo

USER dev
WORKDIR /workspace
