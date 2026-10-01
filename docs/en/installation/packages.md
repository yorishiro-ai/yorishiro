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

## Installed files

| File | Description |
|---|---|
| `/usr/bin/yorishiro` | The binary |
| `/lib/systemd/system/yorishiro.service` | Systemd unit |
| `/etc/yorishiro/yorishiro.yaml` | Editable canonical configuration template |
| `/etc/yorishiro/LICENSE.enterprise` | Enterprise licence |

## Configuration

Edit `/etc/yorishiro/yorishiro.yaml` to configure the packaged service.
Package upgrades preserve local changes to this file.

```yaml
database:
  uri: postgres://user:pass@host:5432/yorishiro
queue:
  kind: Postgres
  uri: postgres://user:pass@host:5432/yorishiro
server:
  host: https://yorishiro.example.com
```

See [docs/configuration.md](../configuration.md) for all settings.

## Start

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl status yorishiro
```
