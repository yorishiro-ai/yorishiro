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
  **CI workflows trigger on `branches: [master]` only** (`ci.yml`, `security.yml`, `cache-cleanup.yml`, `doc-check.yml`, confirmed by grep): a `develop`-based PR runs zero checks until these triggers add `develop`.
  Check this against the actual workflow files before relying on CI to catch anything on a `develop`-based PR.
- Squash merge is the default merge strategy.
- Every PR that changes source code must also update docs (English + Japanese).
  The `doc-check` workflow warns automatically if this is missing.
- Every PR that adds/changes config must update `config.example.yml` and `docs/configuration.md` (English + Japanese).

## Versioning

- `workspace.package.version` in the root `Cargo.toml` is the source of truth.
- 0.x: minor bump = breaking change, patch bump = compatible addition/fix.
- Tag format: `v{version}` (e.g. `v0.8.1`).
  Releases are cut by running the `Release` workflow (`workflow_dispatch` with a `version` input) from the Actions tab or `gh workflow run release.yml -f version=X.Y.Z`, which bumps `Cargo.toml`/`Cargo.lock`, commits, and creates the tag itself.
  Do not hand-edit the version or create the tag locally.
