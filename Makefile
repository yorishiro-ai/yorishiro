# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

# Host-mapped port for a locally run dev Postgres container.
# CI's own DATABASE_URL is set independently in .github/workflows/ci.yml against its Postgres service container, which listens on the standard 5432 inside the job's network namespace rather than this host-mapped 25433 - the two values are deliberately different, not a drift to unify.
DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:25433/yorishiro

ENVIRONMENT ?= development

.PHONY: check clippy fmt fmt-check test build task doctor

check:
	cargo check --workspace

clippy:
	cargo clippy --workspace --tests -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

# make test [ARGS="-p yorishiro-core --test mod auth"]
test:
	DATABASE_URL=$(DATABASE_URL) cargo test --workspace $(ARGS)

build:
	cargo build -p yorishiro-core --bin yorishiro_core-cli

# make task NAME=seed_official_templates [ARGS="key:value"]
task: build
	DATABASE_URL=$(DATABASE_URL) ./target/debug/yorishiro_core-cli task $(NAME) $(ARGS)

# make doctor [ENVIRONMENT=production]
doctor: build
	DATABASE_URL=$(DATABASE_URL) ./target/debug/yorishiro_core-cli doctor -e $(ENVIRONMENT)
