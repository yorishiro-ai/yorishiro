# Rust coding rules for yorishiro

This repository is mid-port from a hand-rolled `sqlx` + `sea-query` + `Engine`-generic data layer to [Loco](https://loco.rs); see `yotsunagi/yorishiro#221` for the rationale.
Open state for the rebuild (what's ported, what's next, what's never been raced) lives in the Yorishiro task list, not here: see `yorishiro-specs/.claude/rules/dogfooding.md` for how to reach it.

## No internal-planning references in this repository

- This repository's source, comments, error messages, PR bodies and commit messages describe only what is true of this repository.
  They never name, quote or point at an internal planning document, its section or step numbers, or its version, whatever form that takes: a design memo, a requirements document, an issue tracker in another repository, or a phrase like "the command post" or "the spec" used to mean one of those.
  A reader of this repository has no way to open that document, so a reference to it explains nothing and leaks a detail about how the work is managed rather than about the code.
- State the constraint itself, in this repository's own words, instead of pointing at where it came from.
  "The Sqlite engine allows only one tenant, since it has no database-enforced isolation to protect" is correct; "see design memo §8" is not, even if both sentences sit next to the same code.
- This also covers Japanese prose outside `docs/ja`, `ee/docs/ja` and a docs-focused PR quoting from one of those files by name.
  Everything else in this repository, including PR bodies and commit messages, is English.
- A public standard is not an internal document: a section reference into an RFC or a similarly public specification (`RFC 6749 §4.1.3`, `OpenID Connect Core §3.1.3.7`) is fine, since any reader can open it.

## Where the rules live

- @.claude/rules/loco-architecture.md repository layout, migrations, models, the SeaORM entity API, the RLS/two-pool `db.rs` architecture.
- @.claude/rules/editions.md the `ee/` boundary, BUSL-1.1 vs. paid, how a feature's edition is decided.
- @.claude/rules/ee-composition.md how `ee/crates/yorishiro-hosted` composes on top of the Loco rebuild (the `Hooks` seam, the licence gate).
- @.claude/rules/error-handling.md `YorishiroError`, `ResultExt`, `into_http_parts()`.
- @.claude/rules/module-structure.md `src/` layout, MCP handlers, router integration, visibility and dead code.
- @.claude/rules/naming-imports.md import grouping, fixed type names.
- @.claude/rules/testing.md the `tests/` integration-test pattern, pool-closing, race-gate pitfalls.
- @.claude/rules/git-workflow.md branching, PR checklist, versioning and releases.
