# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# Backend-specific tests use centralized gates and post-test marker verification.
# Default database URL for PostgreSQL tests.
# Override with: make test-postgres DATABASE_URL=postgres://user:pass@host:port/db
# Targets like `doctor` do not use this default and require an explicit value.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:15432/yorishiro
YORISHIRO_REDIS_TEST_URL ?= redis://localhost:6379/15
export RUSTUP_TOOLCHAIN ?= $(shell sed -n 's/^rust-version *= *"\(.*\)"/\1/p' Cargo.toml)
export CARGO_HOME := $(abspath .cargo-cache)
export CARGO_TARGET_DIR := $(abspath target)
export PATH := $(CARGO_HOME)/bin:$(PATH)

coverage test-postgres task doctor migrate entities: export DATABASE_URL := $(DATABASE_URL)
coverage test-postgres task doctor migrate entities: export LOCO_ENV = test_postgres
coverage test-postgres migrate entities: export DB_MAX_CONNECTIONS ?= 100
coverage test-postgres migrate entities: export DB_CONNECT_TIMEOUT ?= 5000
test-postgres test-sqlite test-redis: export RUST_BACKTRACE ?= 1
test-redis: export YORISHIRO_REDIS_TEST_URL := $(YORISHIRO_REDIS_TEST_URL)

.PHONY: check check-ce check-ee clippy clippy-ce clippy-ee fmt fmt-check python-lint edition-boundary-check dead-code-allow-check test-location-check debug-macro-check coverage test-postgres test-sqlite test-topology test-redis build build-ce build-ee task doctor migrate entities check-all check-all-ee package-docker-check

check: check-ce

.PHONY: patch-check

check-ce: patch-check
	cargo check --locked --no-default-features --features community --workspace

check-ee: patch-check
	cargo check --locked --features enterprise --workspace

clippy: clippy-ce

clippy-ce: patch-check
	cargo clippy --locked --no-default-features --features community --workspace --all-targets -- -D warnings -D clippy::missing_errors_doc -D clippy::missing_panics_doc

clippy-ee: patch-check
	cargo clippy --locked --workspace --all-targets --features enterprise -- -D warnings -D clippy::missing_errors_doc -D clippy::missing_panics_doc

fmt: patch-check
	cargo fmt --all

fmt-check: patch-check
	cargo fmt --all -- --check

python-lint:
	uv run ruff check scripts patches/loco-rs/verify.py
	uv run scripts/test_ci_matrix.py
	uv run scripts/test_classify_ci_changes.py
	uv run scripts/test_topology_self.py
	@for script in .cargo/env.sh scripts/generate_entities.sh .github/packaging/test-artifacts.sh .github/packaging/test-artifacts-self.sh .github/packaging/test-transitions.sh; do bash -n "$$script" || exit; done

edition-boundary-check:
	uv run scripts/test_edition_boundary.py
	uv run scripts/check_edition_boundary.py

dead-code-allow-check:
	@if git grep -n -E 'allow[[:space:]]*\([[:space:]]*dead_code' -- src ee edition migration/src tests; then \
		echo 'ERROR: remove the unused item instead of allowing dead_code' >&2; exit 1; \
	fi

test-location-check:
	@if git grep -n -E '#\[[[:space:]]*((tokio|async_std)::)?test([[:space:]]|\]|\()|#\[[[:space:]]*cfg[[:space:]]*\([[:space:]]*test[[:space:]]*\)' -- src ee edition migration/src; then \
		echo 'ERROR: tests and test-only code belong in tests/' >&2; exit 1; \
	fi

debug-macro-check:
	@if git grep -n -E '(^|[^[:alnum:]_])(dbg|debug_assert|debug_assert_eq|debug_assert_ne)[[:space:]]*!' -- src ee edition migration/src tests; then \
		echo 'ERROR: remove debug-only macros; use tests/ for assertions' >&2; exit 1; \
	fi

# Nightly is required because LLVM branch coverage is not available on stable.
# Artifacts are written to coverage/ (override with COVERAGE_OUTPUT_DIR=...).
coverage: patch-check
	uv run scripts/coverage_report.py

# Run the full suite against the selected backend (postgres by default).
test-postgres: patch-check
	@if [ "$(DATABASE_URL)" = "postgres://yorishiro:yorishiro@localhost:15432/yorishiro" ]; then \
		docker compose up -d --wait testdb; \
	fi
	QUEUE_URL='$(DATABASE_URL)' uv run scripts/test_topology.py postgres-postgres

test-sqlite: patch-check
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' QUEUE_URL='sqlite:///tmp/yorishiro_queue.sqlite3?mode=rwc' LOCO_ENV=test_sqlite uv run scripts/test_topology.py sqlite-sqlite

# Redis DB 15 on the Compose Redis service is reserved for tests.
test-redis: patch-check
	@if [ "$(YORISHIRO_REDIS_TEST_URL)" = "redis://localhost:6379/15" ]; then docker compose up -d --wait redis; fi
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' QUEUE_URL='$(YORISHIRO_REDIS_TEST_URL)' LOCO_ENV=test_sqlite_redis uv run scripts/test_topology.py sqlite-redis

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

check-all: patch-check fmt-check python-lint edition-boundary-check dead-code-allow-check test-location-check debug-macro-check check-ce clippy-ce

check-all-ee: patch-check fmt-check python-lint edition-boundary-check dead-code-allow-check test-location-check debug-macro-check check-ee clippy-ee

package-docker-check:
	@test -n "$(IMAGE)" || { echo 'IMAGE is required' >&2; exit 2; }
	./.github/packaging/test-docker.sh "$(IMAGE)"
