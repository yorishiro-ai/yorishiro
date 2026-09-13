# エンタープライズ設定

[English](../../../docs/en/configuration.md) | **日本語**

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

## 非同期 infer-fill

`PUT /api/workspace/llm-key` でワークスペースの LLM キーを設定してから、`POST /api/schemas/active/{name}/infer-fill` で fill を開始します。
レスポンスには永続化された `job_id` と初期状態 `queued` が含まれます。
読み取り権限を持つキーで `GET /api/inference-jobs/{job_id}` を呼び出し、状態が `completed` または `failed` になるまでポーリングします。
ジョブの状態、適用件数、スキップ件数、シリアライズされたエラーメッセージを保存するため、サーバー再起動後もポーリングできます。
ジョブを claim できるのは状態が `queued` の間だけです。
ワーカーが claim 後にクラッシュした場合、元のワーカーがまだ実行中の可能性があるため、二重推論を避ける目的で永続化された状態は `running` のまま自動再試行されません。
`running` のジョブを再試行する場合は、運用手順で元の実行状態を確認してから対応してください。
ジョブレコードは、将来の保持期間ポリシーが導入されるまで保持されます。

## ワークスペースごとのワーカークラス

`PUT /api/workspace/worker-class` でワークスペースのバックグラウンドジョブを計算リソースに割り当てられます。

| フィールド | 説明 |
|---|---|
| `worker_class` | `tenant_private`、`official`、または `shared` |
