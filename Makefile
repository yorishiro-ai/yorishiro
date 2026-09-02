# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs.
# SQLite tests are gated by require_sqlite_backend() (DATABASE_URL scheme check).
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:25433/yorishiro
LOCO_ENV ?= test_postgres
ENVIRONMENT ?= development

.PHONY: check clippy fmt fmt-check test test-postgres test-sqlite build task doctor

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
	DATABASE_URL=$(DATABASE_URL) \
	LOCO_ENV=$(LOCO_ENV) \
	cargo test --locked --workspace $(ARGS)

test-postgres:
	$(MAKE) test DATABASE_URL=postgres://yorishiro:yorishiro@localhost:25433/yorishiro

test-sqlite:
	$(MAKE) test DATABASE_URL=sqlite://:memory:

build:
	cargo build --locked -p yorishiro --bin yorishiro

# make task NAME=seed_official_templates [ARGS="key:value"]
task: build
	DATABASE_URL=$(DATABASE_URL) \
	LOCO_ENV=$(LOCO_ENV) \
	./target/debug/yorishiro task $(NAME) $(ARGS)

# make doctor [ENVIRONMENT=production]
doctor: build
	DATABASE_URL=$(DATABASE_URL) \
	./target/debug/yorishiro doctor -e $(ENVIRONMENT)

# Convenience alias: check + fmt + clippy (CI check job).
check-all: fmt-check check clippy
