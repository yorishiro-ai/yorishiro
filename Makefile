# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# CI mirrors this structure: check / clippy / fmt-check run once, then test runs
# per-backend via the BACKEND variable (postgres by default).
# YORISHIRO_TEST_BACKEND is set by ci.yml per matrix entry; locally unset to run
# all tests (backend gate falls through to require_sqlite_backend).
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:25433/yorishiro
LOCO_ENV ?= test_postgres
YORISHIRO_TEST_BACKEND ?=
BACKEND ?= postgres
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
# Sets YORISHIRO_TEST_BACKEND so that backend-gated tests only exercise
# their own backend; see tests/mod.rs::require_test_backend().
test:
	DATABASE_URL=$(DATABASE_URL) \
	LOCO_ENV=$(LOCO_ENV) \
	YORISHIRO_TEST_BACKEND=$(YORISHIRO_TEST_BACKEND) \
	cargo test --locked --workspace $(ARGS)

test-postgres:
	$(MAKE) test BACKEND=postgres YORISHIRO_TEST_BACKEND=postgres

test-sqlite:
	$(MAKE) test BACKEND=sqlite YORISHIRO_TEST_BACKEND=sqlite DATABASE_URL=sqlite://:memory:

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
