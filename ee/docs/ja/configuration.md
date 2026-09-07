# エンタープライズ設定

[English](../../docs/configuration.md) | **日本語**

ライセンスキーを必要とする追加設定。

## ワークスペースごとの埋め込みプロバイダ

各ワークスペースごとに埋め込みエンドポイントを割り当てられます。`PUT /api/workspace/embedding-key` で個別の設定が可能です。

| フィールド | 説明 |
|---|---|
| `base_url` | 埋め込みエンドポイントの URL |
| `model` | モデル名 |
| `api_key` | API キー（GET で取得されることはない） |
| `dimensions` | モデルが生成するベクトルの次元数 |
| `send_dimensions_param` | リクエストに dimensions を含めるか（既定：`false`） |

## ワークスペースごとのワーカークラス

`PUT /api/workspace/worker-class` でワークスペースのバックグラウンドジョブを計算リソースに割り当てられます。

| フィールド | 説明 |
|---|---|
| `worker_class` | `tenant_private`、`official`、または `shared` |
