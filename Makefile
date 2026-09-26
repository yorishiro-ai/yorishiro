# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# Backend-specific tests use centralized gates and post-test marker verification.
# Default database URL for PostgreSQL tests.
# Override with: make test-postgres DATABASE_URL=postgres://user:pass@host:port/db
# Targets like `doctor` do not use this default and require an explicit value.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:15432/yorishiro

.PHONY: check clippy fmt fmt-check coverage test-postgres test-sqlite build task doctor entities

check:
	cargo check --locked --workspace

clippy:
	cargo clippy --locked --workspace --tests -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# Nightly is required because LLVM branch coverage is not available on stable.
# Artifacts are written to coverage/ (override with COVERAGE_OUTPUT_DIR=...).
coverage:
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres python3 scripts/coverage_report.py

# Run the full suite against the selected backend (postgres by default).
test-postgres: build
	DATABASE_URL='$(DATABASE_URL)' RUST_BACKTRACE=1 DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres scripts/test-backend.sh postgres

test-sqlite: build
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' RUST_BACKTRACE=1 LOCO_ENV=test_sqlite scripts/test-backend.sh sqlite

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
