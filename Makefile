# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# Tests pick their backend from DATABASE_URL and their edition from the feature set (EDITION=ce|ee, default ee).
# Default database URL for PostgreSQL tests.
# Override with: make test-postgres DATABASE_URL=postgres://user:pass@host:port/db
# Targets like `doctor` do not use this default and require an explicit value.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:15432/yorishiro
EDITION ?= ee
TEST_FEATURES = $(if $(filter ce,$(EDITION)),--no-default-features --features community,--features enterprise)
# rustup reads rust-toolchain.toml; do not override it with Cargo's minimum version.
export CARGO_HOME := $(abspath .cargo-cache)
export CARGO_TARGET_DIR := $(abspath target)
export PATH := $(CARGO_HOME)/bin:$(PATH)

test-postgres task doctor migrate entities: export DATABASE_URL := $(DATABASE_URL)
task doctor migrate entities: export LOCO_ENV = test
test-postgres migrate entities: export DB_MAX_CONNECTIONS ?= 100
test-postgres migrate entities: export DB_CONNECT_TIMEOUT ?= 5000
test-postgres test-sqlite test-valkey: export RUST_BACKTRACE ?= 1

.PHONY: check check-ce check-ee clippy clippy-ce clippy-ee fmt fmt-check python-lint edition-boundary-check test-location-check test-postgres test-sqlite test-valkey build build-ce build-ee task doctor migrate entities check-all check-all-ee package-docker-check

check: check-ce

.PHONY: patch-check

check-ce: patch-check
	cargo check --locked --no-default-features --features community --workspace

check-ee: patch-check
	cargo check --locked --features enterprise --workspace

clippy: clippy-ce

clippy-ce: patch-check
	cargo clippy --locked --no-default-features --features community --workspace --all-targets -- -D warnings

clippy-ee: patch-check
	cargo clippy --locked --workspace --all-targets --features enterprise -- -D warnings

fmt: patch-check
	cargo fmt --all

fmt-check: patch-check
	cargo fmt --all -- --check

python-lint:
	uv run ruff check scripts tests/blackbox tests/ci patches/loco-rs/verify.py
	uv run scripts/test_env_config.py
	@for script in .cargo/env.sh scripts/generate_entities.sh .github/packaging/test-artifacts.sh .github/packaging/test-artifacts-self.sh .github/packaging/test-transitions.sh; do bash -n "$$script" || exit; done

edition-boundary-check:
	uv run scripts/test_edition_boundary.py
	uv run scripts/check_edition_boundary.py

test-location-check:
	@if git grep -n -E '#\[[[:space:]]*((tokio|async_std)::)?test([[:space:]]|\]|\()|#\[[[:space:]]*cfg[[:space:]]*\([[:space:]]*test[[:space:]]*\)' -- src ee edition migration/src; then \
		echo 'ERROR: tests and test-only code belong in tests/' >&2; exit 1; \
	fi

# Every PostgreSQL test creates and drops its own database, so no setup beyond a reachable server is needed.
test-postgres: patch-check
	@if [ "$(DATABASE_URL)" = "postgres://yorishiro:yorishiro@localhost:15432/yorishiro" ]; then \
		docker compose up -d --wait testdb; \
	fi
	QUEUE_URL='$(DATABASE_URL)' cargo test --locked --workspace $(TEST_FEATURES)

test-sqlite: patch-check
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' QUEUE_URL='sqlite:///tmp/yorishiro_queue.sqlite3?mode=rwc' cargo test --locked --workspace $(TEST_FEATURES)

# The Valkey queue (Redis-compatible) next to a SQLite database. Set VALKEY_URL to use another server.
# Only the tests that exercise the provider itself run here; everything else is queue-provider agnostic and runs on the SQL queues.
VALKEY_URL ?= redis://localhost:6379
test-valkey: patch-check
	@if [ "$(VALKEY_URL)" = "redis://localhost:6379" ]; then docker compose up -d --wait redis; fi
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' YORISHIRO_QUEUE_KIND=Redis QUEUE_URL='$(VALKEY_URL)' cargo test --locked --workspace $(TEST_FEATURES) -- workers::dispatch workers::queue::upstream_fixes requests::queue::configured_queue_orders_competing_jobs_by_priority

build: build-ce

build-ce: patch-check
	cargo build --locked --no-default-features --features community --workspace

build-ee: patch-check
	cargo build --locked --features enterprise --workspace

# make task NAME=seed_official_templates [ARGS="key:value"]
task: build
	./target/debug/yorishiro task $(NAME) $(ARGS)

# make doctor DATABASE_URL=postgres://...
doctor: build
ifndef DATABASE_URL
	$(error DATABASE_URL is required for doctor: set it explicitly)
endif
	./target/debug/yorishiro doctor

migrate: build
	docker compose up -d --wait testdb
	./target/debug/yorishiro db migrate

# Generate SeaORM entity structs from the current schema.
# Uses an isolated disposable PostgreSQL container without touching development databases.
entities: build
	@expected=$$(awk '/^name = "sea-orm-cli"$$/ { getline; gsub(/[^0-9.]/, ""); print; exit }' Cargo.lock); \
	actual=$$(sea-orm-cli --version | awk '{print $$2}'); \
	if [ "$$actual" != "$$expected" ]; then \
		echo "ERROR: make entities requires sea-orm-cli $$expected from Cargo.lock, found $$actual; install the pinned CLI before regenerating" >&2; exit 1; \
	fi
	bash scripts/generate_entities.sh

# CE is the base boundary. EE compatibility is checked separately after CE work is complete.
patch-check:
	python3 patches/loco-rs/verify.py --prepare

check-all: patch-check fmt-check python-lint edition-boundary-check test-location-check check-ce clippy-ce

check-all-ee: patch-check fmt-check python-lint edition-boundary-check test-location-check check-ee clippy-ee

package-docker-check:
	@test -n "$(IMAGE)" || { echo 'IMAGE is required' >&2; exit 2; }
	./.github/packaging/test-docker.sh "$(IMAGE)"
