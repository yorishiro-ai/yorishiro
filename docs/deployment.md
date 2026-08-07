# Production Deployment

**English** | [日本語](ja/deployment.md)

For the initial run steps (Docker, prebuilt binary, or from source), see [setup.md](setup.md). This guide covers running the server in the background, cutting releases, and single-tenant mode.

## Running in the background

### Docker

`-d --restart unless-stopped` (used in [setup.md](setup.md#run-with-docker)) runs the container detached and brings it back up after a reboot or crash.

```console
$ docker logs -f yorishiro      # follow output
$ docker stop yorishiro         # graceful shutdown
```

Migrations are embedded in the binary and applied automatically on startup, safe to start multiple replicas concurrently thanks to an advisory lock.

The server shuts down gracefully on SIGTERM/Ctrl-C, waiting for in-flight requests and background embedding syncs to finish (up to 30 seconds) before exiting. If an embedding sync is still lost, recover it with `admin resync-embeddings`.

The admin CLI can be run from the same image:

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

To build the image from source instead (e.g. to test an unreleased change), the same multi-stage `Dockerfile` is at the repository root:

```console
$ docker build -t yorishiro .
```

### systemd (prebuilt binary)

A systemd unit keeps the process from [setup.md](setup.md#run-the-prebuilt-binary) running across reboots and restarts it on failure. Unlike a plain shell, systemd's `EnvironmentFile=` loads `.env` directly, no `source`/`set -a` needed:

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
EnvironmentFile=/opt/yorishiro/.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro
$ journalctl -u yorishiro -f
```

## Releasing

Releases are cut in two stages: bumping the version goes through a PR (`master` is protected by a ruleset that requires PR review, so there's no direct push), and publishing runs automatically once that PR is merged.

1. **Bump.** Run `.github/workflows/release.yml` with a `version` input (e.g. `0.16.3`, without the leading `v`):

   ```console
   $ gh workflow run release.yml -f version=X.Y.Z
   ```

   Or trigger it from the Actions tab (select the `Release` workflow, "Run workflow", enter the version). Its `prepare` job validates the version, checks that the tag doesn't already exist, bumps `workspace.package.version` in the root `Cargo.toml`, runs `cargo update -w` to update `Cargo.lock` accordingly, pushes a `release/vX.Y.Z` branch, and opens a PR titled `Bump version to vX.Y.Z` -- authored by `github-actions[bot]`.

2. **Approve the workflow run.** Because the PR is authored by `github-actions[bot]`, GitHub holds its triggered checks (`check`, `security`, etc.) in `action_required` status until a human approves the run in the Actions tab. Open the run, click "Review pending deployments" / "Approve and run" (wording varies), and approve it -- otherwise the PR's CI never starts. **This step is easy to miss and blocks the whole release if skipped.**

3. **Review and merge.** Once CI is green, approve and squash-merge the bump PR into `master`. **This alone triggers publishing** -- `release-publish.yml` also runs on any push to `master` that touches `Cargo.toml`, so merging the bump kicks off tagging, building, and publishing without a separate manual step.

`release-publish.yml` can still be dispatched manually with a `version` input:

```console
$ gh workflow run release-publish.yml -f version=X.Y.Z
```

This is the recovery path if the automatic run after step 3 didn't fire or failed for some reason, and it's also how to re-run publishing for a version whose auto-triggered run failed partway. On a manual dispatch, `prepare` re-validates the version, fails if the tag already exists, and verifies `master`'s current `Cargo.toml` is actually at `X.Y.Z` (guarding against dispatching before the bump PR merged, or against a later unrelated commit landing on `master` first) before creating and pushing the `vX.Y.Z` tag. On the automatic push-triggered run, the version is read from `master`'s `Cargo.toml` directly, and if a tag for that version already exists -- e.g. the push was a dependency bump or another `Cargo.toml` edit that didn't change the version, not an actual release bump -- the run exits cleanly without tagging or building anything; this is expected and not an error.

Either way, once tagging succeeds, the rest of the workflow builds `yorishiro-server` binaries for `x86_64`/`aarch64` Linux (glibc, packaged as `linux-amd64`/`linux-arm64`) and `x86_64` Windows (packaged as `windows-amd64.zip`), attaches them to a GitHub Release, and builds and pushes a multi-arch Docker image to `ghcr.io/yotsunagi/yorishiro:vX.Y.Z` (and `:latest`). Both Linux architectures build natively (no QEMU), matching the `ort`/onnxruntime build requirements.

## Single-tenant mode

`YORISHIRO_MAX_TENANTS=1` and `YSR_EMBEDDING_PROVIDER=local` (see [configuration.md](configuration.md)) are both defaults. A deployment that leaves them unset already serves the [`web/`](../crates/yorishiro-web/web) SPA compiled into the binary, and embeds using the local ONNX model.

Its setup wizard (see [setup.md](setup.md#first-run-setup)) is enough to onboard the deployment's one tenant. Set `YORISHIRO_MAX_TENANTS=0` to lift the tenant cap instead.
