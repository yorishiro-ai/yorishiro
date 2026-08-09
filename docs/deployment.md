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

A release is one dispatch. `.github/workflows/release.yml` bumps the version, tags it, builds every artifact, verifies the published image boots, and creates the GitHub Release.

```console
$ gh workflow run release.yml -f version=X.Y.Z
```

Or from the Actions tab: select the `Release` workflow, "Run workflow", enter the version without the leading `v`. It must be dispatched from `master` -- the workflow refuses any other ref, since it pushes what it checks out.

What it does, in order:

1. Validates the version is `x.y.z` with no leading zeros, and decides whether this is a fresh release or a resume (see below).
2. Bumps `workspace.package.version` in the root `Cargo.toml`, runs `cargo update -w`, then pushes the bump commit and the `vX.Y.Z` tag to `master` together, atomically.
3. Builds `yorishiro-server` for `x86_64`/`aarch64` Linux (glibc, packaged as `linux-amd64`/`linux-arm64`) and `x86_64` Windows (`windows-amd64.zip`). Both Linux architectures build natively -- no QEMU -- matching the `ort`/onnxruntime build requirements.
4. Builds and pushes a multi-arch Docker image to `ghcr.io/yotsunagi/yorishiro:vX.Y.Z` and `:latest`.
5. **Pulls that published image and boots it against a real PostgreSQL**, failing the release if it does not answer `/up`.
6. Creates the GitHub Release with the binaries attached.

### Recovering from a failed release

The tag lands atomically with the bump in step 2, so a failure in steps 3-5 leaves the tag on `master` with no GitHub Release. **Dispatch the same version again.** The workflow tells the two states apart by whether the GitHub Release exists, not by whether the tag does:

| State | What a dispatch does |
|---|---|
| No tag | Normal release: bump, tag, publish |
| Tag, no Release | Resumes: skips the bump, republishes from the existing tag |
| Tag and Release | Fails loudly -- that version is already out |

The GitHub Release is created last, after every artifact is pushed and the smoke test passes, which is what makes it a reliable marker of "this version shipped". There is no need to delete a tag by hand or burn a patch number.


## Single-tenant mode

`YORISHIRO_MAX_TENANTS=1` and `YSR_EMBEDDING_PROVIDER=local` (see [configuration.md](configuration.md)) are both defaults. A deployment that leaves them unset already serves the [`web/`](../crates/yorishiro-web/web) SPA compiled into the binary, and embeds using the local ONNX model.

Its setup wizard (see [setup.md](setup.md#first-run-setup)) is enough to onboard the deployment's one tenant. Set `YORISHIRO_MAX_TENANTS=0` to lift the tenant cap instead.
