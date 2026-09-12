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

| Workflow | Triggers | Notes |
|---|---|---|
| `ci.yml` | PR push/push to `develop` / `loco-rebuild` | Matrix: postgres + sqlite in parallel |
| `doc-check.yml` | PR push to `develop` / `loco-rebuild` | Warns if source change lacks doc update |
| `security.yml` | PR push to `develop` / `loco-rebuild` | Path-filtered: `Cargo.toml`/`Cargo.lock`/`.cargo/audit.toml` changes only |
| `codeql.yml` | Weekly cron (`0 3 * * 1`) + manual dispatch | Does not run on PR push |
| `dep-audit.yml` | Weekly cron (`0 9 * * 1`) + manual dispatch | Rust dependency audit |
| `windows-weekly.yml` | Weekly cron (`0 18 * * 1`) + manual dispatch | Windows compile check |
| `cache-cleanup.yml` | Closed PR against `develop` | Cleans caches on merged PR branches |
| `release.yml` | Manual dispatch only | `dry_run: true/false` |

Check a workflow's `on:` block before assuming it runs (or doesn't) on a PR.

## Merge strategy

Merge commit is the usual strategy, not squash. PRs that carry an empirical record (actual run's numbers, a confirmed CI log) merge with a merge commit so the record stays in `develop`'s history. Squash for a single mechanical edit (a dependency bump, swapping one CI action).

When unclear, prefer a merge commit: losing an empirical record is the more expensive mistake.

## Docs

Every PR that changes source code must update docs (English + Japanese). The `doc-check` workflow warns if this is missing.

## Versioning

- `version = "0.60.0"` in the root `[package]` is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Tag format: `YYYYMMDD-N` (e.g. `20260830-1`), always a prerelease.
- The `Release` workflow takes only a `dry_run` boolean (manual dispatch from Actions tab, or `gh workflow run release.yml -f dry_run=false`).
- It tags `YYYYMMDD-N` from UTC date and run counter, builds packages and images, and publishes a GitHub Release marked `prerelease: true`.
- Nothing in this workflow edits `Cargo.toml` or commits to `develop`: a prerelease tag is not a SemVer version, and cargo rejects `20260830-1` outright.
- `develop` stays at its SemVer position in the manifest; `yorishiro version` answers with both: `0.60.0 (20260830-1)`.
- Do not hand-edit the version or create the tag locally.
