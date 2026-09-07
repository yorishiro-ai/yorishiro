# Build from Source

## Prerequisites

- Rust 1.97+ (`rustup` recommended)
- Cargo
- For PostgreSQL development: a running PostgreSQL instance with `vector` and `pg_trgm` extensions
- For SQLite development: `sqlite3`

## Build

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make init
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
