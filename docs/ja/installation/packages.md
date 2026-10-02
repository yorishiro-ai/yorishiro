# パッケージインストール（deb / rpm）

prerelease パッケージは、各Dockerイメージと共に[Releases](https://github.com/yorishiro-ai/yorishiro/releases)ページで公開されています。

Ubuntu 24.04 と AlmaLinux 10（glibc 2.39以降）に対応しています。

リリースでは `yorishiro-ce` と `yorishiro-ee` のパッケージを分けて公開します。
CE は `enterprise` Cargo feature なしで、EE は feature 付きでビルドします。
両パッケージは相互に conflict するため、同時にはインストールできません。
もう一方をインストールすると現在のパッケージを置き換え、共有の設定と状態パスは維持されます。

## インストール

```console
$ wget https://github.com/yorishiro-ai/yorishiro/releases/download/<VERSION>/yorishiro-ce-<VERSION>-amd64.deb
$ sudo dpkg -i yorishiro-ce-<VERSION>-amd64.deb
```

## インストール内容（CE）

| ファイル | 説明 |
|---|---|
| `/usr/bin/yorishiro` | バイナリ |
| `/lib/systemd/system/yorishiro.service` | systemdユニット |
| `/lib/systemd/system/yorishiro-worker.service` | タグ付きバックグラウンドワーカーユニット |
| `/etc/yorishiro/yorishiro.yaml` | 編集可能な正規設定テンプレート |

EE パッケージは同じ共有パスに加えて `/etc/yorishiro/LICENSE.enterprise` をインストールします。
CE は EE ライセンスファイルをインストールしません。

## 設定

パッケージ版サービスの設定は `/etc/yorishiro/yorishiro.yaml` を編集します。
パッケージを更新しても、このファイルに加えた変更は保持されます。

```yaml
database:
  uri: postgres://user:pass@host:5432/yorishiro
queue:
  kind: Postgres
  uri: postgres://user:pass@host:5432/yorishiro
server:
  host: https://yorishiro.example.com
```

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。

## 開始

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl enable --now yorishiro-worker
$ sudo systemctl status yorishiro
$ sudo systemctl status yorishiro-worker
```

Loco 1.2.0では、`--server-and-worker`とタグ付きワーカーを組み合わせられないため、ワーカーは別ユニットで起動します。
3つのworker-classタグと`infer-fill`を処理します。
