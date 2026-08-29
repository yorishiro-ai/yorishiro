# Git workflow

`develop` is the mainline and GitHub default branch.

- **Never push directly to develop.**
  All changes go through a PR.
- Branch naming: `feat/<name>`, `fix/<name>`, `docs/<name>`, `refactor/<name>`
- **Before creating a PR branch**, always:
  1. `git fetch origin develop`
  2. `git checkout develop && git pull origin develop`
  3. `git checkout -b <branch-name>` (from up-to-date develop)
- **Before pushing a PR branch**, always:
  1. Run `cargo check`, `cargo clippy -- -D warnings`, `cargo fmt --check` locally
  2. Confirm all pass before pushing
- **Before merging a PR**, always:
  1. Verify all CI checks have passed on the latest commit
  2. If the branch is behind develop, rebase first: `git fetch origin develop && git rebase origin/develop`
- Every PR must pass CI (check + security) before merge.
  **`ci.yml`, `security.yml`, `doc-check.yml`, and `codeql.yml` all trigger on `develop` (and `loco-rebuild`)**, confirmed against each workflow's own `on:` block: a `develop`-based PR runs `check`, `doc-check`, and `Analyze (rust)` on every push, and `security` additionally when `Cargo.toml`/`Cargo.lock` changes (it's path-filtered).
  `cache-cleanup.yml` triggers on a closed PR against `develop`, retargeted from `master` once it became clear master takes no more PRs and the job had therefore stopped firing at all; it cleans up a merged PR branch's caches and does not gate a merge.
  Check a workflow's own `on:` block before assuming it runs (or doesn't) on a `develop`-based PR: this list can drift again.
- Merge commit is the usual strategy, not squash: PRs whose commits carry their own measurement record (an actual run's numbers, a confirmed CI log, an empirical check) merge with a merge commit, so that record stays in `develop`'s history rather than collapsing into a synthesized summary.
  Squash is for a change with no such record to preserve: a single mechanical edit (a dependency bump, swapping one CI action for another) where one clean commit is more useful than the several commits it was made from.
  When unclear which applies, prefer a merge commit: losing an empirical record to a squash is the more expensive mistake.
- Every PR that changes source code must also update docs (English + Japanese).
  The `doc-check` workflow warns automatically if this is missing.
- Every PR that adds/changes config must update `config.example.yml` and `docs/configuration.md` (English + Japanese).

## Versioning

- `workspace.package.version` in the root `Cargo.toml` is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Tag format: `v{version}` (e.g. `v0.8.1`).
  Releases are cut by running the `Release` workflow (`workflow_dispatch` with a `version` input) from the Actions tab or `gh workflow run release.yml -f version=X.Y.Z`, which bumps `Cargo.toml`/`Cargo.lock`, commits, and creates the tag itself.
  Do not hand-edit the version or create the tag locally.
