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

Run `make check-all` for CE changes.
Run `make check-all-ee` when changing EE or shared APIs that EE consumes.
Confirm the relevant backend suites pass before pushing.

## Before merging

1. Verify that all CI checks pass on the latest commit.
2. If behind develop, rebase first: `git fetch origin develop && git rebase origin/develop`

## CI

Install `gh-wait` with `gh extension install k1LoW/gh-wait` and use it instead of `gh run watch`, `gh pr checks --watch`, or a polling loop.
After pushing, identify the workflow run whose `headSha` matches the pushed commit and register one completion rule:

```bash
gh wait workflow <run-id> --repo yorishiro-ai/yorishiro --completed
```

The command returns after it registers a background rule, without waiting for CI.
Do other independent work or yield until a completion event arrives, without repeated status checks, duplicate rules, or repeated waiting replies.
After completion, inspect `gh run view <run-id>` and check the logs for any failures.

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

The server's default polling interval is `1min` for PR/workflow and `30min` for issue/discussion.
Custom interval: `--interval 5min`.

For full workflow reference, see `.github/workflows/`.

## Merge strategy

Prefer a merge commit to keep measurements and CI evidence in `develop`'s history.
Squash a single mechanical change, such as a dependency bump or a CI action update.

## Versioning

- The root `Cargo.toml` package version is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Prerelease tags and asset filenames use `pre-YYYYMMDD-N` (e.g. `pre-20260830-1`).
- The `Release (prerelease)` workflow takes only a `dry_run` boolean.
- `Release (SemVer)` creates `vX.Y.Z` releases and pushes the verified version bump and tag together.
- Prereleases leave `Cargo.toml` unchanged; `yorishiro version` reports both the SemVer version and the prerelease tag.
- Do not hand-edit the version or create the tag locally.
