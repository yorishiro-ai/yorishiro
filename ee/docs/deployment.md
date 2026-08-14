# Hosted Deployment

**English** | [日本語](ja/deployment.md)

`yorishiro-hosted-server` is a single process/binary.
It embeds the full community edition (`yorishiro-server`, from the public [yotsunagi/yorishiro](https://github.com/yotsunagi/yorishiro) repo) as a library -- schemas/entities/search/auth, all of it.
It merges this repo's own routes (Stripe billing, usage metering, the admin dashboard SPA) into the same router.

There is nothing else to run alongside it; `yorishiro-server` itself never starts as a separate process in a hosted deployment.

Community-appropriate defaults that don't fit a hosted, multi-tenant deployment (a single-tenant cap, the first-run setup wizard) are overridden in this binary's own code, not left to environment variables an operator could forget -- see [configuration.md](configuration.md) for what's still configurable.

See [api.md](api.md) for exactly what this repo adds on top of [yotsunagi/yorishiro's docs/api.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/api.md).

Pick one of the two ways to run the server below.

## Run with Docker

Every release publishes `ghcr.io/yotsunagi/yorishiro-hosted:vX.Y.Z` (and `:latest`) — see [Cutting a release](#cutting-a-release) below.
The Docker image builds this repo's admin dashboard SPA (`web/`) in a dedicated Node stage and bundles it at `/app/web` with `YORISHIRO_HOSTED_WEB_DIR` preset, so the enterprise dashboard is served out of the box.

The `web-builder` stage's `node:24-slim` tracks Node 24 (Krypton), the current Active LTS.
Node's majors alternate LTS status -- even ones (24, 26, ...) become LTS, odd ones (25, 27, ...) never do -- and Dependabot's `docker` ecosystem entry can't express "even majors only", so `.github/dependabot.yml` ignores `node`'s major-version bumps entirely (minor/patch still flow through automatically).
Moving to the next even-numbered LTS is a manual `Dockerfile`/`web/package.json` (`engines.node`) update, done the same way as any other deliberate version bump, not something Dependabot will propose on its own.

1. This repo is private, so the GHCR package is private too.
   Log in with a PAT that has `read:packages` and access to this repo:

   ```console
   $ echo "$GITHUB_TOKEN" | docker login ghcr.io -u <github-username> --password-stdin
   ```

2. Start the container:

   ```console
   $ docker run -d --name yorishiro-hosted --restart unless-stopped -p 8081:8081 \
       -e DATABASE_URL=postgres://... \
       -e YORISHIRO_STRIPE_WEBHOOK_SECRET=... \
       -e YORISHIRO_STRIPE_PRICE_PRO=... -e YORISHIRO_STRIPE_PRICE_TEAM=... \
       -e YSR_EMBEDDING_PROVIDER=openai \
       -e YSR_EMBEDDING_BASE_URL=https://api.openai.com/v1 \
       -e YSR_EMBEDDING_MODEL=text-embedding-3-small \
       ghcr.io/yotsunagi/yorishiro-hosted:latest
   ```

   An embedding provider or local ONNX model files are required.
   The default `local` provider loads ONNX model files (`models/model.onnx`, `models/tokenizer.json`) that are **not** bundled in this image, so the process fails at startup before it ever binds a listener unless an embedding provider is set explicitly.
   Use `openai` (with `YSR_EMBEDDING_BASE_URL`/`YSR_EMBEDDING_MODEL` for the endpoint and model you want) or mount a model directory and point `YSR_ONNX_MODEL_PATH`/`YSR_ONNX_TOKENIZER_PATH` at it instead -- see [yotsunagi/yorishiro's docs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md) for the full set of embedding-provider variables.

3. Confirm it's up:

   ```console
   $ curl localhost:8081/up
   ```

`-d --restart unless-stopped` runs it detached and brings it back up after a reboot or crash.
`docker logs -f yorishiro-hosted` follows its output, `docker stop yorishiro-hosted` shuts it down gracefully.
`DATABASE_URL` is this process's own connection -- there's no separate `yorishiro-server` process to share it with.

Migrations -- vendored from the public repo at `vendor/yorishiro/migrations`, then this repo's own `crates/yorishiro-hosted/migrations` -- are applied automatically on startup, safe to run from multiple replicas concurrently (advisory lock).
See [configuration.md](configuration.md) for the full environment variable reference, including the embedded community server's own settings (e.g. `YSR_EMBEDDING_PROVIDER`) which this binary still reads, and the `YORISHIRO_OAUTH_*` variables that enable optional SSO login.

To build the image from source instead (e.g. to test an unreleased change):

```console
$ git submodule update --init
$ docker build -f Dockerfile -t yorishiro-hosted .
$ docker run --rm -p 8081:8081 \
    -e DATABASE_URL=postgres://... \
    -e YORISHIRO_STRIPE_WEBHOOK_SECRET=... \
    -e YORISHIRO_STRIPE_PRICE_PRO=... -e YORISHIRO_STRIPE_PRICE_TEAM=... \
    -e YSR_EMBEDDING_PROVIDER=openai \
    -e YSR_EMBEDDING_BASE_URL=https://api.openai.com/v1 \
    -e YSR_EMBEDDING_MODEL=text-embedding-3-small \
    yorishiro-hosted
```

## Run the prebuilt binary

For a bare-metal or VM deployment without Docker.

1. Download and extract the release archive for your architecture:

   ```console
   $ mkdir -p /opt/yorishiro-hosted && cd /opt/yorishiro-hosted
   $ curl -L -o yorishiro-hosted.tar.gz \
       https://github.com/yotsunagi/yorishiro-enterprise/releases/download/vX.Y.Z/yorishiro-hosted-server-vX.Y.Z-linux-amd64.tar.gz
   $ tar -xzf yorishiro-hosted.tar.gz && rm yorishiro-hosted.tar.gz
   ```

   The archive contains only the `yorishiro-hosted-server` binary.
   This repo's admin dashboard `web/` is **not** bundled with it -- build it separately (`pnpm build` in `web/`) and point `YORISHIRO_HOSTED_WEB_DIR` at the output if you want to serve it (see [web-ui.md](web-ui.md)); without that, `/` is served by the community edition's own embedded assets instead.
2. Create an env file with `DATABASE_URL` and the rest of [configuration.md](configuration.md)'s variables, one `KEY=value` per line:

   ```console
   $ cat > yorishiro-hosted.env <<'EOF'
   DATABASE_URL=postgres://...
   YORISHIRO_STRIPE_WEBHOOK_SECRET=...
   YORISHIRO_STRIPE_PRICE_PRO=...
   YORISHIRO_STRIPE_PRICE_TEAM=...
   YSR_EMBEDDING_PROVIDER=openai
   YSR_EMBEDDING_BASE_URL=https://api.openai.com/v1
   YSR_EMBEDDING_MODEL=text-embedding-3-small
   EOF
   ```

   `YSR_EMBEDDING_PROVIDER` defaults to `local`, which requires ONNX model files this binary doesn't ship with -- see the note in [Run with Docker](#run-with-docker) above; the same requirement applies here.

3. Load it and run:

   ```console
   $ set -a; source yorishiro-hosted.env; set +a
   $ ./yorishiro-hosted-server
   ```

See [Running in the background](#running-in-the-background) below to keep it running across reboots with systemd.

## Running in the background

For a bare-metal/VM deployment, a systemd unit keeps the process from [Run the prebuilt binary](#run-the-prebuilt-binary) running across reboots and restarts it on failure.
Unlike a plain shell, systemd's `EnvironmentFile=` loads the env file directly, no `source`/`set -a` needed.

The unit below runs as a dedicated `yorishiro` system user rather than root.
Create it and hand it ownership of `/opt/yorishiro-hosted` first (the Docker image does the same thing -- see `Dockerfile`'s `useradd --system --no-create-home yorishiro` / `chown -R yorishiro:yorishiro`):

```console
$ sudo useradd --system --no-create-home yorishiro
$ sudo chown -R yorishiro:yorishiro /opt/yorishiro-hosted
```

```ini
# /etc/systemd/system/yorishiro-hosted.service
[Unit]
Description=Yorishiro Hosted server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro-hosted
ExecStart=/opt/yorishiro-hosted/yorishiro-hosted-server
EnvironmentFile=/opt/yorishiro-hosted/yorishiro-hosted.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro-hosted
$ journalctl -u yorishiro-hosted -f
```

## What's embedded vs. what's overridden

`yorishiro-hosted-server` calls the same `build_app`/`build_embedding_provider` functions `yorishiro-server`'s own `main` calls.
The full community REST/MCP/search/auth surface is present and behaves exactly as documented in the public repo, including its own environment variables (`YSR_EMBEDDING_PROVIDER`, `YSR_BIND`-equivalents, etc. -- see [yotsunagi/yorishiro's docs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)).

A couple of things are pinned in code rather than left as operator-set defaults, since a hosted, multi-tenant deployment can never correctly run with the community edition's self-hosted defaults:

- `YORISHIRO_MAX_TENANTS` is force-set to `0` (unlimited) at the very top of `main`, before anything else runs.
  It's not read from the environment, so there's no way to accidentally launch a hosted deployment capped at one tenant.
- Because the tenant cap is unlimited, the community edition's first-run setup wizard (`GET /setup/status` / `POST /setup`) disables itself automatically.
  Tenants here are always provisioned through Stripe checkout or invite redemption instead, never the wizard.
- This binary never reads `YSR_WEB_DIR`.
  It passes its own `YORISHIRO_HOSTED_WEB_DIR` into `build_app` instead, which controls the exact same fallback `YSR_WEB_DIR` controls in `yorishiro-server`'s own `main`: unset falls back to the community edition's own embedded assets (the same ones a self-hosted community deployment would serve), set overrides it with a real directory on disk.
  This repo's own `web/` (the enterprise admin dashboard) is never compiled into the binary itself — it's a separate React SPA built with rsbuild.
  The Docker image builds and bundles it at `/app/web` with `YORISHIRO_HOSTED_WEB_DIR` preset; bare-binary deployments must build it separately and set the variable.
  See [web-ui.md](web-ui.md).

## Admin CLI

The enterprise binary includes the same admin subcommands as the community edition.
Run them with:

```console
$ ./yorishiro-hosted-server admin <command>
```

Available commands: `create-tenant`, `list-tenants`, `create-workspace`, `list-workspaces`, `create-user`, `add-member`, `list-members`, `create-invite`, `create-api-key`, `list-api-keys`, `revoke-api-key`, `resync-embeddings`.

Both vendor (community) and local (enterprise-only) migrations are applied automatically when any admin command runs.
`set_ignore_missing(true)` ensures that each migration runner ignores migration IDs it doesn't own.

See `./yorishiro-hosted-server admin --help` for full usage.

## Onboarding a tenant

Tenant creation and the initial owner account are provisioned exactly as documented in the public repo: admin CLI, or `POST /auth/signup` redeeming an invite (see [yotsunagi/yorishiro's docs/setup.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/setup.md#signup-login-member-and-workspace-management)).

With `YORISHIRO_OAUTH_ISSUER_URL` configured (see [configuration.md](configuration.md#oauth2oidc-login)), a tenant can also onboard itself: the first person from an organization to sign in with SSO gets a brand-new tenant/workspace/`member`-role membership auto-provisioned on the spot, no invite needed.
Every subsequent teammate needs an invite from that first member the same way password-based signup does -- auto-provisioning only fires for an identity provider `sub` this deployment has never seen before, not for every SSO login.

A tenant has no `plan` and no `max_workspaces` cap until Stripe reports a subscription for it (`checkout.session.completed` linking the Stripe customer, then `customer.subscription.created`/`updated` applying the plan) -- see [api.md](api.md#post-hostedstripewebhook).

## Cutting a release

Releases are cut by manually dispatching `.github/workflows/release.yml` from the Actions tab (or `gh workflow run release.yml -f version=X.Y.Z`), passing the new version number without a leading `v` (e.g. `0.12.2`).
`workflow_dispatch` only resolves against whatever `release.yml` looks like on `master` (so a change to the workflow file itself must be merged before it can be dispatched), but the Actions tab still lets you pick which branch or tag to run it *against* -- that ref controls only the initial `prepare` job's checkout.
The workflow's first step refuses to run `prepare` against anything other than `master`, so picking a different branch/tag fails fast instead of silently releasing (and pushing) the wrong content.
Once `prepare` succeeds, the downstream binary/Docker/GitHub-Release jobs check out and build from the `v<version>` tag `prepare` just created, not from whatever ref was originally selected.

The workflow's `prepare` job runs first and does the version bump for you: it validates the version is `x.y.z`-shaped (no leading zeros in any component), decides whether this is a fresh release or a resume (see [Recovering from a failed release](#recovering-from-a-failed-release) below), rewrites `[workspace.package].version` in the root `Cargo.toml`, runs `cargo update -w` to refresh `Cargo.lock` (workspace-only, so this never touches the `vendor/yorishiro`-pinned `yorishiro-core`/`yorishiro-server` git dependencies), then commits both files and pushes the commit plus the `v<version>` tag to `master` directly, atomically.
This is the one place in the repo that pushes to `master` outside a PR -- see [CLAUDE.md](../CLAUDE.md#git-workflow).

Once `prepare` has pushed the tag, the rest of the workflow builds against it exactly as before: `yorishiro-hosted-server` binaries for `x86_64`/`aarch64` Linux (glibc, packaged as `linux-amd64`/`linux-arm64` `.tar.gz`) and `x86_64` Windows (packaged as `windows-amd64` `.zip`), attached to a GitHub Release, plus a multi-arch Docker image pushed to `ghcr.io/yotsunagi/yorishiro-hosted:v<version>` (and `:latest`) -- including a fresh `web/` SPA build baked into the image, since the Docker job also checks out the tagged commit.
The two Linux architectures build natively (no QEMU); the Windows binary is not part of the Docker image.

Because `prepare`'s push uses the workflow's own `GITHUB_TOKEN`, it does not itself re-trigger `ci.yml` on `master` (GitHub doesn't fire `push` events for commits made with the default token) -- this is expected, not a bug.

Before the GitHub Release is created, a `smoke` job pulls the multi-arch manifest that was just published and boots it against a real PostgreSQL, failing the release if it does not answer `/up` on port 8081.
`ci.yml`'s `package-smoke` covers a debug build on every PR, but only this checks the actual release artifact -- including the SPA that the Docker job builds in its own Node stage.

### Recovering from a failed release

`prepare` pushes the bump commit and the tag together, atomically, so a failure in any later job leaves the tag on `master` with no GitHub Release.
**Dispatch the same version again.**
The workflow tells the states apart by whether the GitHub Release exists, not by whether the tag does:

| State | What a dispatch does |
|---|---|
| No tag | Normal release: bump, tag, publish |
| Tag, no Release | Resumes: skips the bump, republishes from the existing tag |
| Tag and Release | Fails loudly -- that version is already out |

The GitHub Release is created last, after every artifact is pushed and the smoke test passes, which is what makes it a reliable marker of "this version shipped".
There is no need to delete a tag by hand or burn a patch number.

## Updating the public-repo dependency version

When the public repo cuts a new tag:

```console
$ cd vendor/yorishiro && git fetch --tags && git checkout <new-tag> && cd ../..
$ # bump both `tag = "..."` entries (yorishiro-core and yorishiro-server) in crates/yorishiro-hosted/Cargo.toml to match
$ cargo update -p yorishiro-core -p yorishiro-server
$ git add vendor/yorishiro crates/yorishiro-hosted/Cargo.toml Cargo.lock
```
