# REST API と MCPツール

[English](../api.md) | **日本語**

## REST API

主なエンドポイント(全一覧と詳細は`/docs`のSwagger UIを参照):

```console
# スキーマ登録(schema scope)
$ curl -X POST localhost:8080/api/schemas \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d @templates/task-management.json

# エンティティ作成(write scope)
$ curl -X POST localhost:8080/api/entities \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d '{"schema_name":"task-management","entity_type":"task","data":{"title":"牛乳を買う"}}'

# ベクトル類似検索(構造化フィルタとの組み合わせ、read scope)
$ curl "localhost:8080/api/search?query_text=買い物&filter=%7B%22status%22%3A%22active%22%7D" \
    -H "Authorization: Bearer $YSR_KEY"

# エンティティとそのリレーション・隣接エンティティを一括取得(read scope)
$ curl "localhost:8080/api/entities/$ENTITY_ID/context" -H "Authorization: Bearer $YSR_KEY"

# ワークスペース全体のJSON Linesエクスポート(read scope)
$ curl "localhost:8080/api/export.jsonl" -H "Authorization: Bearer $YSR_KEY"

# 同じJSON Lines形式を取り込む。単一トランザクションとして実行(schema scope。
# スキーマのインポート自体がschema scope専用の操作であるため)
$ curl -X POST localhost:8080/api/import.jsonl -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/x-ndjson" --data-binary @export.jsonl
```

`GET /api/entities`は`filter`クエリパラメータ(JSONBの包含条件でマッチするJSONオブジェクト、例: `filter={"status":"active"}`)も受け付けます。`POST /api/schemas`はインラインの定義に加えて`{"template_id": "..."}`を渡すことで、`GET /api/templates`で一覧取得できる組み込みテンプレートからスキーマを登録できます。

### `GET /api/templates/{id}`

組み込みテンプレートの完全な定義をIDで取得します(例: `general-notes`)。レスポンスは`MetaSchemaDefinition` JSONオブジェクト — `POST /api/schemas`が受け付けるのと同じ構造です。

### テンプレートライブラリ

上記の組み込みテンプレート(`/api/templates`、読み取り専用、サーバーに同梱)とは別に、各テナントはDBに保存されたテンプレートライブラリを持ち、テンプレートの作成・編集・フォークができます。

| エンドポイント | scope | 内容 |
|---|---|---|
| `GET /api/template-library` | 全メンバー | 呼び出し元のテナントから見えるテンプレート一覧(自身のもの + コミュニティ公開のもの) |
| `GET /api/template-library/{id}` | 全メンバー | 単一テンプレートをIDで取得 |
| `POST /api/template-library` | owner/admin | テンプレートを作成 |
| `PUT /api/template-library/{id}` | owner/admin | テンプレートを更新 |
| `DELETE /api/template-library/{id}` | owner/admin | テンプレートを削除 |
| `POST /api/template-library/{id}/fork` | owner/admin | 既存のテンプレートを新しいテンプレートとしてフォーク |

メンバー/ワークスペース管理と同様、書き込み系エンドポイントはキー自身のscopeとは独立に、呼び出し元のテナントrole(owner/admin)で制御されます。

### 認証・メンバー管理・ワークスペース管理

他の全エンドポイントと異なり、`/auth/signup`と`/auth/login`はbearerトークンを必要としません。これらの目的自体がトークンを発行することだからです。招待からサインアップ・ログインまでの一連の流れは[setup.md](setup.md#サインアップログインメンバーワークスペース管理)を参照してください。

```console
# 招待(`admin create-invite`参照)を引き換えてアカウントを作成
$ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
    -d '{"invite_token":"...","password":"...","display_name":"..."}'

# メールアドレス/パスワードを、新しく発行されたrole上限付きのAPIキーと交換
# workspace_idはアカウントが複数のワークスペースを持つ場合のみ必須(422で指定を促す)
$ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
    -d '{"email":"...","password":"..."}'

# 呼び出し元自身のテナントのメンバーを一覧・追加(owner/adminのみ)
$ curl localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/json" -d '{"email":"...","role":"member"}'

# 呼び出し元自身のテナントのワークスペースを一覧・作成(一覧: 全メンバー、作成: owner/adminのみ)
$ curl localhost:8080/api/workspaces -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/workspaces -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/json" -d '{"name":"staging"}'
```

`POST /api/members`は**既存の**アカウントを呼び出し元のテナントに追加するだけで、新規作成はしません(それはサインアップの役割です)。メンバー管理の両エンドポイントは、キー自身のscopeとは独立に、呼び出し元のテナントrole(owner/admin)で制御されます。ワークスペース管理の`POST`/`DELETE`も同じ規則に従います(一覧取得と`GET /api/workspaces/{id}`による詳細取得はテナントの全メンバーに開放されています)。

`GET /api/workspaces/{id}`のレスポンス(`WorkspaceDetail`)には`schema_id`(UUID、null許容) — このワークスペースに紐づくスキーマ — が含まれます。

## MCPツール

`/mcp`(Streamable HTTP)に接続すると20のツールが使えます。Claude Codeでの接続例:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| ツール | scope | 内容 |
|---|---|---|
| `create_schema` | schema | メタスキーマの登録(新バージョン追加)。インラインの`definition`または`template_id`から作成可能 |
| `list_templates` | read | `create_schema`の`template_id`に指定できる組み込みスキーマテンプレートの一覧 |
| `list_schemas` | read | 登録済みスキーマのサマリ一覧(発見用) |
| `get_active_schema` | read | アクティブなスキーマ定義の取得 |
| `get_schema_by_id` | read | 特定バージョンのスキーマ取得 |
| `get_entity_type_json_schema` | read | entity_typeのJSON Schema投影 |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` | write/read | エンティティCRUD |
| `list_entities` | read | エンティティ一覧。`entity_type`および/または`filter`(JSONB包含マッチ)で絞り込み可能 |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | リレーションCRUD |
| `search_entities` | read | 自然文クエリによるベクトル類似検索。`entity_type`/`filter`で絞り込み可能。埋め込みを持たないエンティティも trigram によるあいまい検索でヒットし得る |
| `recall_context` | read | エンティティとそのリレーション・隣接エンティティを一括取得 |
| `import_jsonl` | schema | エクスポート形式のJSON Linesドキュメントからスキーマ/エンティティ/リレーションを一括インポート。単一トランザクションとして実行 |
| `list_template_library` | read | テナントのDB保存スキーマテンプレートライブラリの一覧(組み込みテンプレートを一覧する`list_templates`とは別物) |
| `get_template_library_item` | read | テナントのDB保存テンプレートライブラリから単一テンプレートをIDで取得 |

REST専用の`GET /api/export.jsonl`エンドポイント(ワークスペース全体のJSON Linesエクスポート)に対応するMCPツールはありませんが、対になる`POST /api/import.jsonl`には上記の`import_jsonl`が対応します。
