# Package Installation (deb / rpm)

Prerelease packages are published on the [Releases](https://github.com/yorishiro-ai/yorishiro/releases) page alongside each Docker image.

They support Ubuntu 24.04 and AlmaLinux 10 (glibc 2.39+).

## Install

```console
$ wget https://github.com/yorishiro-ai/yorishiro/releases/download/20260906-1/yorishiro_20260906-1_amd64.deb
$ sudo dpkg -i yorishiro_20260906-1_amd64.deb
```

## Installed files

| File | Description |
|---|---|
| `/usr/bin/yorishiro` | The binary |
| `/lib/systemd/system/yorishiro.service` | Systemd unit |
| `/etc/yorishiro/yorishiro.env` | Environment file for runtime settings |
| `/usr/share/yorishiro/config/` | Configuration files (not meant to be edited directly) |
| `/etc/yorishiro/LICENSE.enterprise` | Enterprise licence |

## Configuration

Configuration comes from the environment file `/etc/yorishiro/yorishiro.env`:

```yaml
DATABASE_URL=postgres://user:pass@host:5432/yorishiro
YORISHIRO_QUEUE_KIND=Redis
QUEUE_URL=redis://host:6379
YORISHIRO_LICENSE_KEY=...
```

See [docs/configuration.md](../configuration.md) for all settings.

## Start

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl status yorishiro
```
