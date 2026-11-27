# Run from the repository root or use `make -C <repo> <target>` so Loco can find `config/`.

# Set DATABASE_URL to use a different PostgreSQL database for tests or local tasks.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:15432/yorishiro
TEST_COMPOSE := docker compose -f docker-compose.test.yaml
# Select the edition for the test targets.
EDITION ?= ee
TEST_FEATURES = $(if $(filter ce,$(EDITION)),--no-default-features --features community,--features enterprise)
TEST_RUN = $(if $(TEST_BINARY),$(TEST_BINARY),cargo test --locked --workspace $(TEST_FEATURES))
TEST_SEPARATOR = $(if $(TEST_BINARY),,--)
CARGO_TARGET_DIR ?= $(abspath target)

test-postgres task doctor migrate entities: export DATABASE_URL := $(DATABASE_URL)
task doctor migrate entities: export LOCO_ENV = test
test-postgres test-sqlite test-licence test-valkey: export RUST_BACKTRACE ?= 1

.PHONY: check check-ce check-ee clippy clippy-ce clippy-ee fmt fmt-check python-lint edition-boundary-check test-location-check test-postgres test-sqlite test-licence test-valkey build build-ce build-ee task doctor migrate entities check-all check-all-ee package-docker-check patch-check

check: check-ce

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
	uv run ruff check scripts patches/loco-rs/verify.py
	@for script in scripts/generate_entities.sh .github/packaging/test-artifacts.sh .github/packaging/test-artifacts-self.sh .github/packaging/test-package-lifecycle.sh; do bash -n "$$script" || exit; done

edition-boundary-check:
	uv run scripts/check_edition_boundary.py

test-location-check:
	@if git grep -n -E '#\[[[:space:]]*((tokio|async_std)::)?test([[:space:]]|\]|\()|#\[[[:space:]]*cfg[[:space:]]*\([[:space:]]*test[[:space:]]*\)' -- src ee migration/src; then \
		echo 'ERROR: tests and test-only code belong in tests/' >&2; exit 1; \
	fi

# Request tests create disposable databases from template1, which must already contain vector.
test-postgres: $(if $(TEST_BINARY),,patch-check)
	@if [ "$(DATABASE_URL)" = "postgres://yorishiro:yorishiro@localhost:15432/yorishiro" ]; then \
		$(TEST_COMPOSE) up -d --wait testdb; \
	fi
	QUEUE_URL='$(DATABASE_URL)' $(TEST_RUN)

test-sqlite: $(if $(TEST_BINARY),,patch-check)
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' QUEUE_URL='sqlite:///tmp/yorishiro_queue.sqlite3?mode=rwc' $(TEST_RUN)

# Verify that a signed `YORISHIRO_LICENSE_KEY` activates the enterprise edition at boot.
test-licence: override EDITION = ee
test-licence: $(if $(TEST_BINARY),,patch-check)
	@test -n "$$YORISHIRO_LICENSE_KEY" || { echo 'YORISHIRO_LICENSE_KEY is required' >&2; exit 1; }
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' QUEUE_URL='sqlite:///tmp/yorishiro_queue.sqlite3?mode=rwc' $(TEST_RUN) $(TEST_SEPARATOR) --exact ee::controllers::middleware::edition::a_signed_licence_key_activates_enterprise_at_boot

# Run queue-provider tests against Valkey with SQLite for application data.
VALKEY_URL ?= redis://localhost:6379
test-valkey: $(if $(TEST_BINARY),,patch-check)
	@if [ "$(VALKEY_URL)" = "redis://localhost:6379" ]; then $(TEST_COMPOSE) up -d --wait redis; fi
	DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' YORISHIRO_QUEUE_KIND=Redis QUEUE_URL='$(VALKEY_URL)' $(TEST_RUN) $(TEST_SEPARATOR) workers::dispatch workers::queue::upstream_fixes ee::workers::queue::configured_queue_orders_competing_jobs_by_priority

build: build-ce

build-ce: patch-check
	cargo build --locked --no-default-features --features community --workspace

build-ee: patch-check
	cargo build --locked --features enterprise --workspace

# Pass task arguments with NAME=<task> and ARGS="key:value".
task: build
	./target/debug/yorishiro task $(NAME) $(ARGS)

# make doctor DATABASE_URL=postgres://...
doctor: build
ifndef DATABASE_URL
	$(error DATABASE_URL is required for doctor: set it explicitly)
endif
	./target/debug/yorishiro doctor

migrate: build
	@if [ "$(DATABASE_URL)" = "postgres://yorishiro:yorishiro@localhost:15432/yorishiro" ]; then \
		$(TEST_COMPOSE) up -d --wait testdb; \
	fi
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

patch-check:
	python3 patches/loco-rs/verify.py --prepare

# CI also checks the enterprise overlay separately.
check-all: patch-check fmt-check python-lint edition-boundary-check test-location-check check-ce clippy-ce

check-all-ee: patch-check fmt-check python-lint edition-boundary-check test-location-check check-ee clippy-ee

package-docker-check:
	@test -n "$(IMAGE)" || { echo 'IMAGE is required' >&2; exit 2; }
	./.github/packaging/test-docker.sh "$(IMAGE)"
