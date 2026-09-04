# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# SQLite tests are gated by require_sqlite_backend() (DATABASE_URL scheme check).
# Default database URL for PostgreSQL tests.
# Override with: make test DATABASE_URL=postgres://user:pass@host:port/db
# Targets like `doctor` do not use this default and require an explicit value.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:5432/yorishiro
LOCO_ENV ?= test_postgres
ENVIRONMENT ?= development

.PHONY: check clippy fmt fmt-check test test-postgres test-sqlite build task doctor fetch entities

check:
	cargo check --locked --workspace

clippy:
	cargo clippy --locked --workspace --tests -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# Run the full suite against the selected backend (postgres by default).
# SQLite tests are gated by require_sqlite_backend() which checks the
# DATABASE_URL scheme; set DATABASE_URL to an SQLite URL to run them.
test:
	DATABASE_URL='$(DATABASE_URL)' \
	LOCO_ENV='$(LOCO_ENV)' \
	cargo test --locked --workspace $(ARGS)

test-postgres:
	$(MAKE) test DATABASE_URL='$(DATABASE_URL)'

test-sqlite:
	$(MAKE) test DATABASE_URL='sqlite://:memory:'

build:
	cargo build --locked -p yorishiro --bin yorishiro

# make task NAME=seed_official_templates [ARGS="key:value"]
task: build
	DATABASE_URL=$(DATABASE_URL) \
	LOCO_ENV=$(LOCO_ENV) \
	./target/debug/yorishiro task $(NAME) $(ARGS)

# make doctor ENVIRONMENT=production DATABASE_URL=postgres://...
doctor: build
ifndef DATABASE_URL
	$(error DATABASE_URL is required for doctor: set it explicitly)
endif
	DATABASE_URL='$(DATABASE_URL)' \
	./target/debug/yorishiro doctor -e $(ENVIRONMENT)

# Warm the cargo registry cache without building.
# Useful after clearing ~/.cargo/registry so the next check/build does not steal download time.
fetch:
	cargo fetch --locked

# Generate SeaORM entity structs from the current schema.
# Starts a disposable pgvector/pgvector:pg18 container on port 15433,
# runs migrations, generates entities, tears everything down.
# No DATABASE_URL needed — no risk of pointing at SQLite or a stale database.
# --rm on the container means stopping it also removes it; no leftover.
entities: build
	docker compose up -d testdb
	@sleep 10
	DATABASE_URL=postgres://yorishiro:yorishiro@localhost:15433/yorishiro DB_CONNECT_TIMEOUT=5000 ./target/debug/yorishiro db migrate
	rm -f src/models/_entities/*.rs
	DATABASE_URL=postgres://yorishiro:yorishiro@localhost:15433/yorishiro DB_CONNECT_TIMEOUT=5000 ./target/debug/yorishiro db entities
	docker compose down -v testdb

# Convenience alias: check + fmt + clippy (CI check job).
check-all: fmt-check check clippy
