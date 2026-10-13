# Docker インストール

Yorishiro は1つのバイナリで動作します。Docker ならすぐに試せます。

## クイックスタート

既定では SQLite を使うため、外部データベースは不要です。

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

1. `http://localhost/`（ホスト側ポート `80`、コンテナ側ポート `5150`）にアクセスし、セットアップウィザードでアカウントを作成します。
2. API キーを発行して、エンティティを作成します。

## PostgreSQL に切り替える

マルチテナントで運用する場合は PostgreSQL を指定します。

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## Valkey (Redis) に切り替える

キューに Valkey を使う場合は接続先を指定します。

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -e YORISHIRO_QUEUE_KIND=Redis \
    -e QUEUE_URL=redis://user:pass@host:6379 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## ボリューム永続化

コンテナを削除してもデータを残すには、ボリュームをマウントします。

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -v yorishiro-data:/var/lib/yorishiro \
    -v yorishiro-model-cache:/home/yorishiro/.cache/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## 設定

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。
イメージは `/app/config/production.yaml` を所有し、実行時に読み込みます。
このパスへ置き換えファイルをマウントするか、`LOCO_CONFIG_FOLDER` で別の設定ディレクトリを指定してください。
