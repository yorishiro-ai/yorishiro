# Package Installation (deb / rpm)

Prerelease packages are published on the [Releases](https://github.com/yorishiro-ai/yorishiro/releases) page alongside each Docker image.

They support Ubuntu 24.04 and AlmaLinux 10 (glibc 2.39+).

Releases publish separate `yorishiro-ce` and `yorishiro-ee` packages.
CE is built without the `enterprise` Cargo feature, while EE includes it and requires an EE licence for production use.
The packages conflict with each other, so both editions cannot be installed at once.
Installing the other edition replaces the current package and preserves the shared configuration and state paths.

## Install

```console
$ wget https://github.com/yorishiro-ai/yorishiro/releases/download/<VERSION>/yorishiro-ce-<VERSION>-amd64.deb
$ sudo dpkg -i yorishiro-ce-<VERSION>-amd64.deb
```

## Installed files (CE)

| File | Description |
|---|---|
| `/usr/bin/yorishiro` | The binary |
| `/var/lib/yorishiro/worker-wrapper.sh` | Wrapper that discovers tags at boot |
| `/lib/systemd/system/yorishiro.service` | Systemd unit |
| `/lib/systemd/system/yorishiro-worker.service` | Tagged background worker unit |
| `/etc/yorishiro/production.yaml` | Editable Loco production configuration |

The EE package installs the same shared paths and additionally installs `/etc/yorishiro/LICENSE.enterprise`.
CE does not install the enterprise licence file.

## Configuration

Edit `/etc/yorishiro/production.yaml` to configure the packaged service.
Package upgrades preserve local changes to this file.
The application database can use SQLite or PostgreSQL, while queue storage is independently selected through Loco's queue provider.

```yaml
database:
  uri: postgres://user:pass@host:5432/yorishiro
queue:
  kind: Postgres
  uri: postgres://user:pass@host:5432/yorishiro
server:
  host: https://yorishiro.example.com
```

For a Redis-compatible queue service, set `YORISHIRO_QUEUE_KIND=Redis` and its `QUEUE_URL` in the service environment instead of using the PostgreSQL queue block.

See [docs/configuration.md](../configuration.md) for all settings.

## Start

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl enable --now yorishiro-worker
$ sudo systemctl status yorishiro
$ sudo systemctl status yorishiro-worker
```

The worker is a separate unit because Loco 1.2.0 cannot combine `--server-and-worker` with tagged workers.
The wrapper script calls `yorishiro worker-tags` at boot to discover every registered worker tag and passes them as a comma-separated list to `start --worker=`, so the list stays correct when new worker classes or job types are added.

## Server-only mode

In server-only mode (`yorishiro start` with no `--worker` flag) the local embedding model is not loaded, which saves memory and startup time.
Search and recall fail loudly with a helpful error if no external embedding provider (such as `YORISHIRO_EMBEDDING_BASE_URL`) is configured.
Any mode that dequeues or executes embedding jobs loads the model as before.

## Directory integrity

The state directory `/var/lib/yorishiro` is owned by `root:yorishiro` with mode `1770` (sticky bit + group-writable).
The `yorishiro` user can create and update state files inside the directory, but cannot replace or rename root-owned files such as `worker-wrapper.sh`.
This prevents an account compromise from replacing the tag-discovery wrapper with a script that exfiltrates state.
The sticky bit ensures each user can only remove its own files, even though the directory is group-writable.
