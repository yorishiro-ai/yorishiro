# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# SQLite tests are gated by require_sqlite_backend() (DATABASE_URL scheme check).
# Default database URL for PostgreSQL tests.
# Override with: make test DATABASE_URL=postgres://user:pass@host:port/db
# Targets like `doctor` do not use this default and require an explicit value.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:15432/yorishiro

.PHONY: check clippy fmt fmt-check test-postgres test-sqlite build task doctor entities

check:
	cargo check --locked --workspace

clippy:
	cargo clippy --locked --workspace --tests -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# Run the full suite against a disposable testdb container (port 15432).
# docker compose up -d testdb starts the container; init-extensions.sql
# installs vector + pg_trgm into both the target database and template1
# so that Loco's CREATE DATABASE-based test harness works.
#
# Override DATABASE_URL to point at a different Postgres instance.
test-postgres: build
	@if ! docker inspect -f '{{.State.Health.Status}}' testdb 2>/dev/null | grep -q healthy; then \
		docker compose up -d testdb; \
		@echo "Waiting for testdb to become healthy..."; \
		for i in $$(seq 1 60); do \
			if docker inspect -f '{{.State.Health.Status}}' testdb 2>/dev/null | grep -q healthy; then break; fi; \
			sleep 1; \
		done; \
		if [ "$$(docker inspect -f '{{.State.Health.Status}}' testdb 2>/dev/null)" != '"healthy"' ]; then \
			echo "ERROR: testdb did not become healthy" >&2; exit 1; \
		fi; \
	fi
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres cargo test --locked --workspace -- --test-threads=1

test-sqlite: build
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' LOCO_ENV=test_sqlite cargo test --locked --workspace -- --test-threads=1

build:
	cargo build --locked --workspace

# make task NAME=seed_official_templates [ARGS="key:value"]
task: build
	DATABASE_URL='$(DATABASE_URL)' LOCO_ENV=test_postgres ./target/debug/yorishiro task $(NAME) $(ARGS)

# make doctor DATABASE_URL=postgres://...
doctor: build
ifndef DATABASE_URL
	$(error DATABASE_URL is required for doctor: set it explicitly)
endif
	DATABASE_URL='$(DATABASE_URL)' LOCO_ENV=test_postgres ./target/debug/yorishiro doctor

# Generate SeaORM entity structs from the current schema.
# Starts a disposable pgvector/pgvector:pg18 container on port 15432,
# runs migrations, generates entities, tears everything down.
entities: build
	docker compose up -d testdb
	@sleep 10
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres ./target/debug/yorishiro db migrate
	rm -f src/models/_entities/*.rs
	@if echo '$(DATABASE_URL)' | grep -q '^sqlite://'; then echo "ERROR: entities requires PostgreSQL (DATABASE_URL must start with postgres://)" >&2; exit 1; fi
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres ./target/debug/yorishiro db entities
	docker compose down -v testdb

# Convenience alias: check + fmt + clippy (CI check job).
check-all: fmt-check check clippy
