# パッケージインストール（deb / rpm）

プレリリース版のパッケージは、Docker イメージとともに [Releases](https://github.com/yorishiro-ai/yorishiro/releases) で公開します。

Ubuntu 24.04 と AlmaLinux 10（glibc 2.39以降）に対応しています。

Community 用の `yorishiro-ce` と Enterprise 用の `yorishiro-ee` から選べます。
Enterprise の本番利用にはライセンスが必要です。
両パッケージは同時にはインストールできません。
もう一方をインストールすると現在のパッケージを置き換えます。

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
| `/etc/yorishiro/production.yaml.example` | アップグレードごとに更新される最新の本番用デフォルト設定 |
| `/etc/yorishiro/production.yaml` | 任意の管理者所有オーバーライド |

EE パッケージは同じ共有パスに加えて `/etc/yorishiro/LICENSE.enterprise` をインストールします。
CE は EE ライセンスファイルをインストールしません。

## 設定

`/etc/yorishiro/production.yaml` がない場合、サービスは package の `/etc/yorishiro/production.yaml.example` を使います。既定では `/var/lib/yorishiro` 配下の SQLite database と queue file を使います。
Tera の `get_env` 式により、`DATABASE_URL`、`QUEUE_URL`、`HOST`、`PORT`、`BINDING` などの環境変数で template の既定値を上書きできます。
管理者による上書きが必要な場合のみ `/etc/yorishiro/production.yaml` を作成してください。
パッケージのアップグレードでは `/etc/yorishiro/production.yaml.example` が最新のデフォルト設定に更新されます。
管理者所有オーバーライドを作成・変更する前に、package のデフォルト設定を確認してください。
ファイルが存在する場合は、アップグレード前に次のコマンドでバックアップしてください。

```console
$ sudo cp -a /etc/yorishiro/production.yaml /etc/yorishiro/production.yaml.backup
```

CE/EE の edition 切り替えでは、両 package とも同じ example path を管理し、管理者オーバーライドの path は所有・導入しません。
Community Edition は `YORISHIRO_MAX_TENANTS` や YAML の設定にかかわらず、テナント数を1に制限します。異なる値が指定されると警告します。
Enterprise Edition は有効期限内の有効なライセンスがある場合のみ `YORISHIRO_MAX_TENANTS` を適用し、`0` は無制限です。ライセンスがない場合は警告して1テナントに制限します。
どちらの Edition でも SQLite は1テナントに制限されます。
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
