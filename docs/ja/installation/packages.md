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
| `/var/lib/yorishiro/worker-wrapper.sh` | 起動時にタグを自動検出するラッパースクリプト |
| `/lib/systemd/system/yorishiro.service` | systemdユニット |
| `/lib/systemd/system/yorishiro-worker.service` | タグ付きバックグラウンドワーカーユニット |
| `/etc/yorishiro/example.yaml` | `config/production.yaml` から配布する参照設定 |

EE パッケージは同じ共有パスに加えて `/etc/yorishiro/LICENSE.enterprise` をインストールします。
CE は EE ライセンスファイルをインストールしません。

## 設定

サービスを有効化または起動する前に、`/etc/yorishiro/example.yaml` を
`/etc/yorishiro/production.yaml` へコピーして、管理者所有のコピーを編集してください。
パッケージは `/etc/yorishiro/production.yaml` を所有・インストールせず、upgrade と removal でも変更しません。
パッケージを更新しても、このファイルに加えた変更は保持されます。
アプリケーションデータベースは SQLite または PostgreSQL を利用でき、キューの保存先は Loco のキュープロバイダとして独立して選択できます。

```yaml
database:
  uri: postgres://user:pass@host:5432/yorishiro
queue:
  kind: Postgres
  uri: postgres://user:pass@host:5432/yorishiro
server:
  host: https://yorishiro.example.com
```

Redis 互換キューサービスを使う場合は、PostgreSQL のキューブロックの代わりにサービス環境へ `YORISHIRO_QUEUE_KIND=Redis` と接続先の `QUEUE_URL` を設定します。

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。

## 開始

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl enable --now yorishiro-worker
$ sudo systemctl status yorishiro
$ sudo systemctl status yorishiro-worker
```

Loco 1.2.0では、`--server-and-worker`とタグ付きワーカーを組み合わせられないため、ワーカーは別ユニットで起動します。
ラッパースクリプトは起動時に `yorishiro worker-tags` を実行して登録済みのすべてのタグを自動検出するため、新しいワーカークラスやジョブタイプが追加されてもリストは常に正しく保たれます。

## ディレクトリの整合性

状態ディレクトリ `/var/lib/yorishiro` の所有者は `root:yorishiro`、モード `1770`（sticky bit + グループ書込可能）です。
`yorishiro` ユーザはディレクトリ内で状態ファイルの作成・更新ができますが、`worker-wrapper.sh` などの root 所有ファイルの置き換えや名前変更はできません。
これにより、アカウント侵害時に状態の漏洩を目的としたラッパースクリプトへの置き換えが防止されます。
sticky bit により、ディレクトリが書込可能でも各ユーザは自分のファイルしか削除できません。
