# パッケージインストール（deb / rpm）

プレリリース版のパッケージは、Docker イメージとともに [Releases](https://github.com/yorishiro-ai/yorishiro/releases) で公開します。

Ubuntu 24.04 と AlmaLinux 10（glibc 2.39以降）に対応しています。

Community 用の `yorishiro-ce` と Enterprise 用の `yorishiro-ee` から選べます。
Enterprise の本番利用にはライセンスが必要です。
両パッケージは同時にはインストールできません。
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
| `/etc/yorishiro/example.yaml` | `config/production.yaml` から配布する参照設定 |

EE パッケージは同じ共有パスに加えて `/etc/yorishiro/LICENSE.enterprise` をインストールします。
CE は EE ライセンスファイルをインストールしません。

## 設定

サービスを有効化または起動する前に、`/etc/yorishiro/example.yaml` を `/etc/yorishiro/production.yaml` にコピーし、管理者所有のファイルを編集してください。
パッケージは `/etc/yorishiro/production.yaml` をインストールしないため、更新や削除でもこのファイルは残ります。
アプリケーションのデータベースには SQLite または PostgreSQL を使えます。キューの保存先は別に選べます。

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

設定項目は [設定](../configuration.md) を参照してください。

## 開始

```console
$ sudo systemctl enable --now yorishiro
$ sudo systemctl enable --now yorishiro-worker
$ sudo systemctl status yorishiro
$ sudo systemctl status yorishiro-worker
```

ワーカーは別のサービスとして動き、`worker-target:shared` を処理します。
ほかの対象を処理するには、それぞれのサービスを設定してください。

## ディレクトリの整合性

状態ディレクトリ `/var/lib/yorishiro` の所有者は `root:yorishiro`、モード `1770`（sticky bit + グループ書込可能）です。
`yorishiro` ユーザはディレクトリ内で状態ファイルの作成・更新ができますが、root 所有ファイルの置き換えや名前変更はできません。
sticky bit により、ディレクトリが書込可能でも各ユーザは自分のファイルしか削除できません。
