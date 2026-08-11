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

# 新規ワークスペースはスキーマを持たないため、最初の1本ができるまでエンティティ作成は422で
# 拒否される(「create a schema first: POST /api/schemas...」)。最初のスキーマ作成で解除され、
# ワークスペースの`status`が schema_pending から active になる。

# リレーションを削除せずに引退させる。探索は辿らなくなるが記録は残る(write scope)。
# status は active / deprecated / archived の3値。
$ curl -X PUT "localhost:8080/api/relations/$RELATION_ID/status" \
    -H "Authorization: Bearer $YSR_KEY" -H "Content-Type: application/json" \
    -d '{"status": "deprecated"}'

# 特定の状態のリレーションのみを一覧する。`status`を省略すると全状態が対象(read scope)
$ curl "localhost:8080/api/relations?status=active" -H "Authorization: Bearer $YSR_KEY"

# エンティティとそのリレーション・隣接エンティティを一括取得(read scope)
$ curl "localhost:8080/api/entities/$ENTITY_ID/context" -H "Authorization: Bearer $YSR_KEY"

# JSON Linesエクスポート: テナント内の全スキーマバージョン + このワークスペースのエンティティ・
# リレーション(read scope)
$ curl "localhost:8080/api/export.jsonl" -H "Authorization: Bearer $YSR_KEY"

# 同じJSON Lines形式を取り込む。単一トランザクションとして実行(schema scope。
# スキーマのインポート自体がschema scope専用の操作であるため)
$ curl -X POST localhost:8080/api/import.jsonl -H "Authorization: Bearer $YSR_KEY" \
    -H "Content-Type: application/x-ndjson" --data-binary @export.jsonl
```

`GET /api/entities`は`filter`クエリパラメータ(JSONBの包含条件でマッチするJSONオブジェクト、例: `filter={"status":"active"}`)と`schema_version`クエリパラメータも受け付けます。`POST /api/schemas`はインラインの定義に加えて`{"template_id": "..."}`を受け付けます。

`template_id`は2種類のテンプレートをどちらも受け付けます。呼び出し側はidがどちらの種類かを知る必要がありません。

| 形式 | 解決先 | 一覧取得 |
|---|---|---|
| `"task-management"` | バイナリに同梱された組み込みテンプレート | `GET /api/templates` |
| UUID | そのテナント自身のテンプレートライブラリ | `GET /api/template-library` |

どちらを引くかはパース結果で決まります。UUIDはライブラリのみ、それ以外は組み込みのみを検索します。他テナントのライブラリテンプレートは`404`を返します。存在しない場合と同じ応答であり、差分から存在を確認できません。

`schema_version`は、そのバージョンのスキーマに対して作成されたエンティティのみを返します。エンティティは作成時のスキーマバージョンを記録し、新しいバージョンが作成された後もその値を保持するため、これは「そのバージョンが生成したエンティティ」を返します。「現在そのバージョンで検証を通るエンティティ」ではありません。

リクエストボディはすべて2 MiB上限です(超えると`413 Payload Too Large`)。大きめのエクスポートを`POST /api/import.jsonl`で取り込む際に関係します。

### `GET /api/templates/{id}`

組み込みテンプレートの完全な定義をIDで取得します(例: `general-notes`)。レスポンスは`MetaSchemaDefinition` JSONオブジェクト — `POST /api/schemas`が受け付けるのと同じ構造です。

### テンプレートライブラリ

上記の組み込みテンプレート(`/api/templates`、読み取り専用、サーバーに同梱)とは別に、各テナントはDBに保存されたテンプレートライブラリを持ち、テンプレートの作成・編集・フォークができます。

| エンドポイント | scope | 内容 |
|---|---|---|
| `GET /api/template-library` | 有効なAPIキー | 呼び出し元のテナントから見えるテンプレート一覧(自身のもの + コミュニティ公開のもの) |
| `GET /api/template-library/{id}` | 有効なAPIキー | 単一テンプレートをIDで取得 |
| `POST /api/template-library` | owner/admin | テンプレートを作成 |
| `PUT /api/template-library/{id}` | owner/admin | テンプレートを更新 |
| `DELETE /api/template-library/{id}` | owner/admin | テンプレートを削除 |
| `POST /api/template-library/{id}/fork` | owner/admin | 既存のテンプレートを新しいテンプレートとしてフォーク |

読み取り系エンドポイントは当該テナントの有効なAPIキーであれば呼び出せます(それ以上のテナントメンバーシップチェックはありません)。メンバー/ワークスペース管理と同様、書き込み系エンドポイントはさらにキー自身のscopeとは独立に、呼び出し元のテナントrole(owner/admin)で制御されます。

フォークは元のテンプレートを記録するだけの独立したコピーなので、フォーク元のテンプレートを削除しても成功します — フォーク自体はそのまま有効なまま残り、削除された元テンプレートへの参照だけが失われます。

### テンプレートマーケットプレイス

テンプレートをテナント間で共有します。`identity.templates` は既に `visibility`(`tenant` | `community`)と `fork_of` を持っており、マーケットプレイスはその上に「共有されたテンプレートを安全に使うための情報」——公開済みバージョンと、他テナントの評価——を載せます。

| エンドポイント | scope | 内容 |
|---|---|---|
| `GET /api/marketplace` | 有効なAPIキー | 全テナント横断の公開テンプレート一覧。最新stableバージョンとレビュー集計を含む |
| `GET /api/marketplace/{id}/versions` | 有効なAPIキー | 公開済みバージョン(新しい順)。draftは自テナント所有のテンプレートのみ含む |
| `POST /api/marketplace/{id}/versions` | 有効なAPIキー | 自テナントのテンプレートの次バージョンを公開(`definition`、任意の`changelog`、`status`は`draft`/`pre`/`stable`) |
| `GET /api/marketplace/{id}/reviews` | 有効なAPIキー | 閲覧できるテンプレートのレビュー一覧 |
| `POST /api/marketplace/{id}/reviews` | 有効なAPIキー | 自テナントのレビューを投稿・更新(`rating` 1-5、任意の`comment`) |
| `POST /api/marketplace/{id}/fork?version=N` | 有効なAPIキー | 公開済みバージョンを自テナントのライブラリへコピー。`version`省略時は最新の`stable` |
| `PUT /api/marketplace/{id}/visibility` | 有効なAPIキー | 自テナントのテンプレートを公開する/取り下げる |

バージョン番号はサーバーがテンプレートごとに採番します。クライアントに選ばせると、他テナントが履歴として読む連番に欠番や衝突が生じるためです。

**draftは所有テナントにのみ見え**、forkもできません。また非draftのバージョンが1つも無いテンプレートは一覧にも現れません。導入しようとすると404になるエントリが並ぶ一覧は、短い一覧より役に立たないためです。forkしたコピーは自テナントのライブラリに**非公開**で作成されます。他者の成果物を自分の名前で再公開するかどうかは、既定ではなく判断だからです。

自テナントが所有しないテンプレートへの操作は`403`ではなく`404`を返します。操作できない呼び出し元が、レスポンスの違いからテンプレートの存在を確認できてはならないためです。

**forkはテンプレートであって、まだスキーマではありません。** スキーマにするには `POST /api/schemas` に そのUUIDを `template_id` として渡します(組み込みidと同じ扱いです)。

#### 公式listing

組み込みテンプレートもここに公開されます。公開は `yorishiro-server admin seed-official-templates` が行います。これらは通常のlistingであり、他と同様にforkもレビューもできます。author は `Yorishiro` です。

公開元は**メンバーもワークスペースも持たない**テナント行です。`identity.templates.tenant_id` は `NOT NULL` であり、マーケットプレイスは所有権をこの列で判定するため、公式listingにも所有者が必要になります。ログインの経路となるメンバーシップが存在しないため、このテナントにログインすることは誰にもできません。

このコマンドは冪等であり、毎回のデプロイで実行することを想定しています。同じ定義で公開済みのテンプレートには何もせず、新しいリリースで定義が変わったものは**新しいバージョンとして公開**します。既にテナントが導入した版を書き換えることはありません。

### 認証・メンバー管理・ワークスペース管理

`/auth/signup`と`/auth/login`はbearerトークンを必要としません。これらの目的自体がトークンを発行することだからです。`/setup`/`/setup/status`([setup.md](setup.md#初回セットアップ)参照)と、生存確認・準備確認用の`/up`/`/health`も同様に認証不要です。このうち入力を受け付ける4つ(`/auth/signup`、`/auth/login`、`/setup`、`/setup/status`)は呼び出し元IPベースでレート制限されます(上限を超えると`429 Too Many Requests`。[configuration.md](configuration.md)の`YSR_AUTH_RATE_LIMIT_MAX`/`YSR_AUTH_RATE_LIMIT_WINDOW_SECS`参照) — 生存確認用の`/up`/`/health`はレート制限の対象外です。招待からサインアップ・ログインまでの一連の流れは[setup.md](setup.md#サインアップログインメンバーワークスペース管理)を参照してください。

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

`GET /api/workspaces/{id}`のレスポンス(`WorkspaceDetail`)には`schema_id`(UUID、null許容) — このワークスペースに紐づくスキーマ — が含まれます。テナントに残る最後の1ワークスペースへの`DELETE`は`409 Conflict`で拒否されます。

### 認証によるキー解決の差し替え

`authenticate`はこのクレート自身の規則です。提示されたキーは、そのキーに記録された1つのワークスペースへ解決され、リクエストのヘッダは結果に影響しません。別の規則を必要とするデプロイ——ワークスペースをリクエストごとに指定するキー、外部のID基盤が発行したキー、このクレートが知らないクレームを持つキー——は`yorishiro_core::services::auth::Authenticator`を実装し、`AppState::with_authenticator`で差し込みます。

認証を要する全経路がこの1つの値を経由します。`AuthContext`・`Authorized<R>`・`Verified<R>`の各抽出子と、MCPの2つの入口です。したがって差し替えはプロセス全体の認証を変更するのであって、「参照することを覚えていた経路」だけが変わるのではありません。RESTのルートとMCPのツールが呼び出し元の identity について食い違うことは起こりません。

実装はリクエストのヘッダをそのまま受け取るため、キー自体が持たない情報を読めます。以下2点は残りの仕組みが前提とするため、実装が必ず守る必要があります。

- 検証できないキーは、コンテキストを返さず`YorishiroError::Unauthenticated`で拒否する
- 返すコンテキストの`tenant_id`がその`workspace_id`を所有していること。RLSのセッション変数は両方から設定されるため、不整合な組は「あるテナントのワークスペースを別テナントのポリシー下で読むセッション」を生む

返されたコンテキストに対してscope判定は引き続き行われるため、認証の差し替えが認可の回避手段になることはありません。

## 未マッチのパス(Web UIのフォールバック)

上記のAPIルートに一致しないリクエストパスは、すべてWeb UIの静的ファイルサーバーにフォールバックします。その挙動はパスが「ファイルらしく見えるか」によって変わります。

- 拡張子なし(例: `/foo`、`/dashboard`、`/schemas/abc`) — 常にSPAの`index.html`を`200 OK`で返します。これによりWeb UIのクライアントサイドルーティングが機能します — 認識できないパスは全て「存在しないリソース」ではなく「SPAのルート」とみなされます。
- 拡張子あり(例: `/foo.js`、`/does-not-exist.txt`) — 該当ファイルが存在すれば返し(組み込み、または`YSR_WEB_DIR`設定時はそこから)、存在しなければSPAフォールバックなしの正真正銘の`404 Not Found`を返します。

そのため、拡張子のないパスはこのフォールバックを通じて404になることが決してありません — タイプミスしたAPIルート(例: `GET /api/entitites`)は`404`のJSONエラーではなくSPAのHTMLを返すため、クライアント側のデバッグ時に混乱の原因になり得ます。ドットファイル形式のパス(例: `/.env`)も拡張子なし扱いとなり、単純な404ではなく`index.html`にフォールバックします — 先頭のドットは拡張子の区切りではなくファイル名の一部として扱われるため、`Path::extension()`(およびこのフォールバックロジック)からは拡張子なしに見えます。

## MCPツール

`/mcp`(Streamable HTTP)に接続すると21のツールが使えます。Claude Codeでの接続例:

```console
$ claude mcp add --transport http yorishiro http://localhost:8080/mcp \
    --header "Authorization: Bearer $YSR_KEY"
```

| ツール | scope | 内容 |
|---|---|---|
| `create_schema` | schema | メタスキーマの登録(新バージョン追加)。インラインの`definition`または`template_id`から作成可能 |
| `list_templates` | read | `create_schema`の`template_id`に指定できる組み込みスキーマテンプレートの一覧(テンプレートライブラリのUUIDも指定可能) |
| `list_schemas` | read | 登録済みスキーマのサマリ一覧(発見用) |
| `get_active_schema` | read | アクティブなスキーマ定義の取得 |
| `get_schema_by_id` | read | 特定バージョンのスキーマ取得 |
| `get_entity_type_json_schema` | read | entity_typeのJSON Schema投影 |
| `create_entity` / `get_entity` / `update_entity` / `delete_entity` | write/read | エンティティCRUD |
| `list_entities` | read | エンティティ一覧。`entity_type`、`filter`(JSONB包含マッチ)、`schema_version`で絞り込み可能 |
| `create_relation` / `get_relation` / `delete_relation` / `list_relations` | write/read | リレーションCRUD |
| `set_relation_status` | write | リレーションを `active` / `deprecated` / `archived` へ遷移させる。グラフ探索は `active` のみを辿るため、「存在した」という記録を残したまま関係を引退させられる |
| `search_entities` | read | 自然文クエリによるベクトル類似検索。`entity_type`/`filter`で絞り込み可能。埋め込みを持たないエンティティも trigram によるあいまい検索でヒットし得る |
| `recall_context` | read | エンティティとそのリレーション・隣接エンティティを一括取得 |
| `import_jsonl` | schema | エクスポート形式のJSON Linesドキュメントからスキーマ/エンティティ/リレーションを一括インポート。単一トランザクションとして実行 |
| `list_template_library` | read | テナントのDB保存スキーマテンプレートライブラリの一覧(組み込みテンプレートを一覧する`list_templates`とは別物) |
| `get_template_library_item` | read | テナントのDB保存テンプレートライブラリから単一テンプレートをIDで取得 |

REST専用の`GET /api/export.jsonl`エンドポイント(テナント内の全スキーマバージョン + ワークスペースのエンティティ・リレーションをJSON Linesで出力)に対応するMCPツールはありませんが、対になる`POST /api/import.jsonl`には上記の`import_jsonl`が対応します。
