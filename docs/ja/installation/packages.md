# パッケージインストール（deb / rpm）

prerelease パッケージは、各Dockerイメージと共に[Releases](https://github.com/yorishiro-ai/yorishiro/releases)ページで公開されています。

Ubuntu 24.04 と AlmaLinux 10（glibc 2.39以降）に対応しています。

## インストール

```console
$ wget https://github.com/yorishiro-ai/yorishiro/releases/download/20260906-1/yorishiro_20260906-1_amd64.deb
$ sudo dpkg -i yorishiro_20260906-1_amd64.deb
```

## インストール内容

| ファイル | 説明 |
|---|---|
| `/usr/bin/yorishiro` | バイナリ |
| `/lib/systemd/system/yorishiro.service` | systemdユニット |
| `/etc/yorishiro/yorishiro.env` | 環境設定ファイル |
| `/usr/share/yorishiro/config/` | 設定ファイル（直接編集するものではありません） |
| `/etc/yorishiro/LICENSE.enterprise` | エンタープライズライセンス |

## 設定

設定は環境ファイル `/etc/yorishiro/yorishiro.env` で行います：

```yaml
DATABASE_URL=postgres://user:pass@host:5432/yorishiro
YORISHIRO_QUEUE_KIND=Redis
QUEUE_URL=redis://host:6379
YORISHIRO_LICENSE_KEY=...
```

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。

## 開始

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl status yorishiro
```
