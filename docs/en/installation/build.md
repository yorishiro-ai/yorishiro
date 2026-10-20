# Build from Source

## Prerequisites

- Rust 1.94 is the supported MSRV. Normal production builds use the pinned Rust 1.97 toolchain (`rustup` recommended).
- Cargo
- A C compiler for the enterprise JWT backend (AWS-LC); supported native targets use the crate's pregenerated bindings.
- For PostgreSQL development: a running PostgreSQL instance with `vector` and `pg_trgm` extensions
- For SQLite development: `sqlite3`

## Build

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make build
```

## Run

```console
$ cargo run -- start
```

## Release build

```console
$ cargo build --release --bin yorishiro
```

The binary is at `target/release/yorishiro`.

Enterprise licence and OIDC RS256 verification use `jsonwebtoken`'s AWS-LC backend, not its RustCrypto RSA backend.
This removes the `rsa` dependency affected by RUSTSEC-2023-0071 rather than suppressing its advisory.
The accepted RS256 algorithms and licence/OIDC claim validation are unchanged.

## Run with PostgreSQL

```console
$ DATABASE_URL=postgres://user:pass@host:5432/yorishiro cargo run -- start
```

## Run with Valkey queue

```console
$ DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
  YORISHIRO_QUEUE_KIND=Redis \
  QUEUE_URL=redis://host:6379 \
  cargo run -- start
```

See [docs/configuration.md](../configuration.md) for all settings.
