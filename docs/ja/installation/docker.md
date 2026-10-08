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
    -v yorishiro-data:/var/lib/yorishiro \
    -v yorishiro-model-cache:/home/yorishiro/.cache/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## 設定

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。
イメージには参照用設定 `/app/config/example.yaml` だけが含まれます。
`/app/config/production.yaml` と `/app/config/production.local.yaml` がなければ、バイナリに埋め込まれた `config/example.yaml` と同じバイト列を使います。
ファイルで設定する場合は、完全な管理者所有の `/app/config/production.yaml` と必要に応じて `/app/config/production.local.yaml` を mount するか、これらを含むディレクトリを `LOCO_CONFIG_FOLDER` に指定してください。
イメージは mount された設定ファイルを作成、merge、書換え、chmod、削除しません。
参照用既定値は `127.0.0.1` に bind します。Docker Compose は `5150` を公開するため `BINDING=0.0.0.0` を明示します。
