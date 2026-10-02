# Git workflow

`develop` is the mainline and GitHub default branch.

## Branching

- **Never push directly to develop.** All changes go through a PR.
- Branch naming: `feat/<name>`, `fix/<name>`, `docs/<name>`, `refactor/<name>`
- **Before creating a PR branch:**
  1. `git fetch origin develop`
  2. `git checkout develop && git pull origin develop`
  3. `git checkout -b <branch-name>`

## Before pushing

Run `make check-all` (`fmt-check + check + clippy`). Confirm all pass before pushing.

## Before merging

1. Verify all CI checks passed on the latest commit.
2. If behind develop, rebase first: `git fetch origin develop && git rebase origin/develop`

## CI

Wait for PR/CI events using `gh-wait` (`gh extension install k1LoW/gh-wait`):

**Wait for CI completion:**

```bash
gh wait pr <number> --ci-completed
```

**Wait for PR approval:**

```bash
gh wait pr <number> --approved --open
```

**Wait for merge:**

```bash
gh wait pr <number> --merged --open
```

**Wait for CI failure:**

```bash
gh wait pr <number> --ci-failed --open
```

**Continuous watch (e.g., notify on every new comment until PR is closed):**

```bash
gh wait pr <number> --commented --open --until closed
```

**Manage rules:**

```bash
gh wait list              # List all watch rules
gh wait delete <id>       # Delete a specific rule
gh wait delete --all      # Delete all rules
```

Polling interval defaults to `1min` for PR/workflow and `30min` for issue/discussion. Custom interval: `--interval 5min`.

For full workflow reference, see `.github/workflows/`.

## Merge strategy

Merge commit is the usual strategy, not squash. PRs that carry an empirical record (actual run's numbers, a confirmed CI log) merge with a merge commit so the record stays in `develop`'s history. Squash for a single mechanical edit (a dependency bump, swapping one CI action).

When unclear, prefer a merge commit: losing an empirical record is the more expensive mistake.

## Docs

Every PR that changes source code must update docs (English + Japanese). The `doc-check` workflow warns if this is missing.

## Versioning

- `version = "0.60.0"` in the root `[package]` is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Tag format: `YYYYMMDD-N` (e.g. `20260830-1`), always a prerelease.
- The `Release (prerelease)` workflow takes only a `dry_run` boolean and publishes date-based `YYYYMMDD-N` tags as prereleases.
- The separate `Release` workflow creates SemVer `vX.Y.Z` releases and uses the version bump prepared by its release branch.
- Nothing in the prerelease workflow edits `Cargo.toml` or commits to `develop`: a date tag is not a SemVer version, and cargo rejects `20260830-1` outright.
- `develop` stays at its SemVer position in the manifest for prereleases; `yorishiro version` answers with both versions.
- Do not hand-edit the version or create the tag locally.
