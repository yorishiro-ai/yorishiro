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

Releases are cut by running `.github/workflows/release.yml` via `workflow_dispatch` with a `version` input (e.g. `0.16.3`, without the leading `v`) -- there is no tag-push trigger. The workflow's `prepare` job validates the version, checks that the tag doesn't already exist, bumps `workspace.package.version` in the root `Cargo.toml`, runs `cargo update -w` to update `Cargo.lock` accordingly, commits both to `master`, and creates and pushes the `vX.Y.Z` tag itself -- there is no need to (and no longer a way to) bump the version or create the tag by hand.

```console
$ gh workflow run release.yml -f version=X.Y.Z
```

Or trigger it from the Actions tab on GitHub (select the `Release` workflow, "Run workflow", enter the version).

Once the version is bumped and tagged, the rest of the workflow builds `yorishiro-server` binaries for `x86_64`/`aarch64` Linux (glibc, packaged as `linux-amd64`/`linux-arm64`) and `x86_64` Windows (packaged as `windows-amd64.zip`), and attaches them to a GitHub Release.

It also builds and pushes a multi-arch Docker image to `ghcr.io/yotsunagi/yorishiro:vX.Y.Z` (and `:latest`). Both architectures build natively (no QEMU), matching the `ort`/onnxruntime build requirements.

## Single-tenant mode

`YORISHIRO_MAX_TENANTS=1` and `YSR_EMBEDDING_PROVIDER=local` (see [configuration.md](configuration.md)) are both defaults. A deployment that leaves them unset already serves the [`web/`](../crates/yorishiro-web/web) SPA compiled into the binary, and embeds using the local ONNX model.

Its setup wizard (see [setup.md](setup.md#first-run-setup)) is enough to onboard the deployment's one tenant. Set `YORISHIRO_MAX_TENANTS=0` to lift the tenant cap instead.
