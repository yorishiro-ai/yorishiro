# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# Backend-specific tests use centralized gates and post-test marker verification.
# Default database URL for PostgreSQL tests.
# Override with: make test-postgres DATABASE_URL=postgres://user:pass@host:port/db
# Targets like `doctor` do not use this default and require an explicit value.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:15432/yorishiro
YORISHIRO_REDIS_TEST_URL ?= redis://localhost:6379/15

.PHONY: check check-ce check-ee clippy clippy-ce clippy-ee fmt fmt-check python-lint public-api-check edition-boundary-check coverage test-postgres test-sqlite test-topology build build-ce build-ee task doctor migrate entities check-all check-all-ee package-docker-check

check: check-ce

check-ce:
	cargo check --locked --no-default-features --features community --workspace

check-ee:
	cargo check --locked --features enterprise --workspace

clippy: clippy-ce

clippy-ce:
	cargo clippy --locked --no-default-features --features community --workspace --all-targets -- -D warnings -D clippy::missing_errors_doc -D clippy::missing_panics_doc

clippy-ee:
	cargo clippy --locked --workspace --all-targets --features enterprise -- -D warnings -D clippy::missing_errors_doc -D clippy::missing_panics_doc

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

python-lint:
	uv run ruff check scripts

public-api-check:
	PYTHONDONTWRITEBYTECODE=1 uv run scripts/test_public_api.py
	PYTHONDONTWRITEBYTECODE=1 uv run scripts/check_public_api.py
	@test -z "$$(find scripts -type f \( -name '*.pyc' -o -path '*/__pycache__/*' \) -print -quit)"

edition-boundary-check:
	PYTHONDONTWRITEBYTECODE=1 uv run scripts/test_edition_boundary.py
	PYTHONDONTWRITEBYTECODE=1 uv run scripts/check_edition_boundary.py
	@test -z "$$(find scripts -type f \( -name '*.pyc' -o -path '*/__pycache__/*' \) -print -quit)"

# Nightly is required because LLVM branch coverage is not available on stable.
# Artifacts are written to coverage/ (override with COVERAGE_OUTPUT_DIR=...).
coverage:
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres uv run scripts/coverage_report.py

# Run the full suite against the selected backend (postgres by default).
test-postgres: build
	@if [ "$(DATABASE_URL)" = "postgres://yorishiro:yorishiro@localhost:15432/yorishiro" ]; then \
		docker compose up -d --wait testdb; \
	fi
	DATABASE_URL='$(DATABASE_URL)' QUEUE_URL='$(DATABASE_URL)' YORISHIRO_TEST_TOPOLOGY=postgres-postgres YORISHIRO_TEST_QUEUE=postgres RUST_BACKTRACE=1 DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres uv run scripts/test_topology.py postgres-postgres

test-sqlite: build
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' QUEUE_URL='sqlite:///tmp/yorishiro_queue.sqlite3?mode=rwc' YORISHIRO_TEST_TOPOLOGY=sqlite-sqlite YORISHIRO_TEST_QUEUE=sqlite RUST_BACKTRACE=1 LOCO_ENV=test_sqlite uv run scripts/test_topology.py sqlite-sqlite

# Redis DB 15 is reserved for tests because Loco clears the selected DB.
# The default endpoint is provisioned locally by Compose. An explicit endpoint
# uses the caller's service instead and does not start a local container.
test-redis:
	@if [ "$(YORISHIRO_REDIS_TEST_URL)" = "redis://localhost:6379/15" ]; then \
		docker compose up -d --wait redis; \
	fi
	YORISHIRO_REDIS_TEST_URL='$(YORISHIRO_REDIS_TEST_URL)' uv run scripts/test_redis.py

build: build-ce

build-ce:
	cargo build --locked --no-default-features --features community --workspace

build-ee:
	cargo build --locked --features enterprise --workspace

# make task NAME=seed_official_templates [ARGS="key:value"]
task: build
	DATABASE_URL='$(DATABASE_URL)' LOCO_ENV=test_postgres ./target/debug/yorishiro task $(NAME) $(ARGS)

# make doctor DATABASE_URL=postgres://...
doctor: build
ifndef DATABASE_URL
	$(error DATABASE_URL is required for doctor: set it explicitly)
endif
	DATABASE_URL='$(DATABASE_URL)' LOCO_ENV=test_postgres ./target/debug/yorishiro doctor

migrate: build
	docker compose up -d --wait testdb
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres ./target/debug/yorishiro db migrate

# Generate SeaORM entity structs from the current schema.
# Starts a disposable pgvector/pgvector:pg18 container on port 15432,
# runs migrations, generates entities, tears everything down.
entities: build
	docker compose up -d --wait testdb
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres ./target/debug/yorishiro db migrate
	rm -f src/models/_entities/*.rs
	@if echo '$(DATABASE_URL)' | grep -q '^sqlite://'; then echo "ERROR: entities requires PostgreSQL (DATABASE_URL must start with postgres://)" >&2; exit 1; fi
	DATABASE_URL='$(DATABASE_URL)' DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres ./target/debug/yorishiro db entities
	uv run scripts/harden_generated_entities.py
	docker compose down -v testdb

# CE is the base boundary. EE compatibility is checked separately after CE work is complete.
check-all: fmt-check python-lint public-api-check edition-boundary-check check-ce clippy-ce

check-all-ee: fmt-check python-lint public-api-check edition-boundary-check check-ee clippy-ee

package-docker-check:
	@test -n "$(IMAGE)" || { echo 'IMAGE is required' >&2; exit 2; }
	./packaging/test-docker.sh "$(IMAGE)"
