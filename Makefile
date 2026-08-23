# Run from anywhere with `make -C <this directory> <target>`: recipes run with this Makefile's directory as CWD, which is exactly what a Loco task needs (`Config::from_folder` always resolves a bare relative "config" against CWD, per CLAUDE.md's Loco rebuild notes).

DATABASE_URL ?= postgres://yorishiro:yorishiro@localhost:25433/yorishiro

.PHONY: check clippy fmt fmt-check test task

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

# make task NAME=seed_official_templates [ARGS="key:value"]
task:
	DATABASE_URL=$(DATABASE_URL) ./target/debug/yorishiro_core-cli task $(NAME) $(ARGS)
