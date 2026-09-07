# Docker インストール

Yorishiroは1つのバイナリとして提供されます。Dockerが最も簡単な方法です。

## クイックスタート

デフォルト設定はSQLiteを使用しており、外部データベースは不要です。

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

1. `http://localhost/` にアクセスし、セットアップウィザードでアカウントを作成。
2. APIキーを生成し、エンティティの作成を開始。

## PostgreSQL に切り替える

マルチテナントホスティングやベクトル検索：

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## Valkey (Redis) に切り替える

分散キュー処理：

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -e YORISHIRO_QUEUE_KIND=Redis \
    -e QUEUE_URL=redis://user:pass@host:6379 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## ボリューム永続化

デフォルトではDockerはコンテナ内部にデータを保存します。永続化する場合：

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -v yorishiro-data:/home/yorishiro/.cache/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## 設定

全設定は [docs/ja/configuration.md](configuration.md) を参照してください。
production設定ファイルはイメージ内の `/app/config/production.yaml` にあります。
