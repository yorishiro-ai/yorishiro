# 設定

[English](../en/configuration.md) | **日本語**

Yorishiro は環境変数で設定します。設定用のファイルは不要です。

## ワーカークラスの admission limit

これらの上限は同じ application DB を共有する全プロセスで、対象ごとの `shared:shared`、`official:official`、`tenant_private:tenant:<tenant-id>` のキーを使います。
Shared と Official はデプロイ全体、tenant-private は tenant 全体の上限であり、workspace ごとの上限ではありません。
plan はライフサイクルのメタデータとして残り、キーには入りません。
Enterprise の plan policy は既定で無効です。
有効にした場合、Official ジョブは自身の plan に応じた上限を一つの共通 Official 実行数と比較し、plan ごとに実行数を分けません。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_WORKER_CLASS_SHARED_LIMIT` | shared class の同時実行上限（default: `1`、`1`以上） |
| `YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT` | tenant-private class の同時実行上限（default: `1`、`1`以上） |
| `YORISHIRO_WORKER_OFFICIAL_LIMIT` | official class の任意の同時実行上限。未設定時は Community が `1`、Enterprise が plan 値の Free `1`、Pro `4`、Team `8` を使用します。設定時は両 edition を上書きし、`1`以上が必要です |

## 埋め込み（自動検索）

Yorishiro はテキストを数値に変換し、類似検索を有効にできます。これを「埋め込み」と呼びます。

### 仕組み

エンティティを作成または更新すると、Yorishiro はバックグラウンドで自動的に埋め込みを生成します。保存が完了した後に実行されるので、書き込みが遅くなることはありません。

埋め込みには2つの経路があります：

**ドキュメント埋め込み（バックグラウンド）。** 埋め込み同期ワーカー（`workers::embedding_sync`）はバックグラウンドジョブとして動作し、エンティティ書き込み後にベクトルを保存します。これが `entity_embeddings` にベクトルを保存する唯一の経路です。

**クエリ埋め込み（ワーカー経由）。** 検索エンドポイントは短時間だけ有効なクエリ埋め込みリクエストを保存し、設定された上限時間内にポーリングします。クエリ埋め込みワーカーがプロバイダを呼び出して結果を保存します。サーバーはプロバイダを読み込まず、待機中にデータベース接続を保持しません。

同じワークスペースでは、ドキュメントとクエリの埋め込みは同じ設定済みプロバイダとモデルを解決します。異なるワークスペースでは、異なるプロバイダ、モデル、対応する次元数を使用できます。

ワーカーがタイムアウトした場合やプロバイダが失敗した場合、検索は `503` を返します。字句検索だけに黙ってフォールバックすることはありません。

ただし例外があります：バックアップファイル（`import_jsonl`）から復元した場合、インポートされたエンティティには埋め込みがありません。単語を直接一致させるあいまい検索で検索は可能ですが、埋め込みが生成されるまで類似検索の結果は不完全になります。すべてのエンティティの埋め込みを生成するには次のコマンドを実行してください：

```
cargo loco task resync_embeddings workspace_id:<uuid>
```

埋め込みがないエンティティが原因でエラーが発生することはありません。検索結果の精度が下がるだけです。

### 埋め込みプロバイダの選択

マシン上で動くローカルモデルか、OpenAI 互換のサービス（LM Studio、Ollama、vLLM、OpenAI など）のどちらかを選べます。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `none` で埋め込みを無効化、`local` でローカルモデルを使用。`YORISHIRO_EMBEDDING_BASE_URL` と `YORISHIRO_EMBEDDING_MODEL` の両方を設定した場合、こちらより優先して OpenAI 互換プロバイダが使われる |
| `YORISHIRO_EMBEDDING_BASE_URL` | OpenAI 互換の埋め込みエンドポイントの URL（例：`http://localhost:11434`）。`YORISHIRO_EMBEDDING_MODEL` とセットで設定する |
| `YORISHIRO_EMBEDDING_MODEL` | OpenAI 互換プロバイダのモデル名 |
| `YORISHIRO_EMBEDDING_API_KEY` | OpenAI 互換プロバイダの API キー。ローカルサービスでは空のままにする |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | ベクトルの次元数（既定：`768`）。使うモデルに合わせる |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | リクエストに `dimensions` フィールドを含めるか。既定は `false` |

OpenAI 互換プロバイダは、ドメイン境界として公開の `EmbeddingProvider` 契約を維持します。
ネットワークを使わずに失敗をテストできるよう、外部リクエストはプロバイダ固有の非公開 transport seam の内側に置いています。
これにより、埋め込みの動作を変更せず、アプリケーション全体で共有する別の HTTP サービスも公開しません。
Loco にはアプリケーション構成の拡張点はありますが、このプロバイダ固有の外向きリクエストを管理する拡張点はありません。

### ローカルモデル

`YORISHIRO_EMBEDDING_PROVIDER=local` でマシン上でモデルを実行します。

使えるモデルは 1 つだけです：`multilingual-e5-base`（768次元）。日本語を含む多言語に対応しています。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_LOCAL_MODEL` | モデル名（既定：`multilingual-e5-base`）。認識できない値を指定すると起動が失敗する |
| `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` | 入力の最大トークン数（既定：`512`） |

**モデルのダウンロード。** モデルファイルは約 1 GB あり、初めて使うときに自動的にダウンロードされます。2 回目以降はキャッシュされたコピーを使います。`models/<short_id>/` にファイルを手動で置くこともできます。モデルファイルとトークナイザファイルの両方が必要です。

ダウンロードに失敗した場合は再起動すれば再度試行されます。バイナリには組み込みのチェックサムがあり、ダウンロードしたファイルが検証されます。

ダウンロードにはローカルの非公開 artifact transport seam を使います。
一方、埋め込み artifact コンポーネントが、固定した URL とリビジョン、タイムアウト、期待する長さ、SHA-256 検証、部分ファイルの削除、アトミックな rename をまとめて管理します。
このキャッシュはアプリケーションの保存領域ではなくデプロイ先のローカルファイルシステムに置く検証済み artifact キャッシュです。検証済みのバイト列を保存先と同じファイルシステム上で一時配置してからアトミックに rename する必要があるため、Loco の storage 拡張点は適用しません。

### 埋め込みモデルの変更

デプロイメント全体でサポートする次元数は `768`、`1024`、`1536` の固定集合です。
マイグレーションは、現在どのワークスペースでも使っていない場合を含め、3 つの幅のパーティションをあらかじめ作成します。
これにより、ワークスペースごとのプロバイダ選択、認証情報のローテーション、リインデックスで実行時のテーブル作成やマイグレーション競合が発生せず、異なる幅のワークスペースを同時に運用できます。
未使用パーティションのコストは小さなカタログとインデックスのメタデータだけで、ベクトルの容量はデータが入ったパーティションにだけ発生します。
ワークスペースとプロバイダの設定はこの固定集合から 1 つの幅を選び、`reindex_embeddings` が既存ベクトルを新しいモデルまたは幅へ移します。

別の埋め込みモデルに変更した場合、既存のエンティティは旧モデルのベクトルのままになります。影響を受けるワークスペースの埋め込み再生成が必要です：

```
cargo loco task reindex_embeddings workspace_id:<uuid>
```

これによりワークスペース内の全エンティティが新しいモデルで再埋め込みされます。リインデックスが完了するまでの間、そのワークスペースへの新規書き込みは拒否されます。

API からもリインデックスを登録できます：`POST /api/migration-jobs/reindex`（Migration スコープが必要）。起動時にワークスペースのモデル変更を自動検知し、リインデックスも走ります。

PostgreSQL では、テナントプールのワークスペース単位 advisory lock により再インデックスの実行を直列化します。SQLite では、単一テナントのバックエンド仕様に従い、ワーカーがモデル所有の再インデックス経路を直接実行します。

埋め込み同期は、意図的にキュー上で 1 回だけ実行します。固定している Loco 1.2.0 のキュー API には、すべてのバックエンドで使える遅延 enqueue がなく、失敗を直ちに再 enqueue すると無制限ループになり、ライフサイクル更新とキュー更新もアトミックにコミットできません。ベクトルの永続化を確認できた場合だけ成功とし、無効化または未設定の埋め込みは明示的な no-op、enqueue 後の削除は成功する no-op、古いスナップショットは新しいエンティティ更新によって terminal に superseded と記録します。プロバイダ、resolver、データベース、バリデーション、構造上の失敗は failed として記録し Loco に返し、自動 enqueue は行いません。別途設計した scheduler ができるまでは、明示的な resync または reindex で復旧します。

**失敗した埋め込みジョブの運用上の境界。** `queue_job_lifecycles` の行が正となるのは、Loco がジョブのペイロードをデシリアライズでき、かつ有効な `lifecycle_id` を含んでいる場合に限ります。UUID の不正、未知の `worker_class`、必須フィールドの欠落などでデシリアライズに失敗したペイロードは、埋め込み処理に入る前に Loco のキューランナーが拒否します。この失敗はキュープロバイダの失敗ジョブにのみ記録され、ライフサイクルの行は `queued` のまま残ることがあります。確認するには、プロバイダの失敗ジョブ（キューテーブルの `JobStatus::Failed` の行、Redis の場合は対応する failed セット）を参照してください。型付きの埋め込み実行エラーはこれと異なり、ライフサイクルの行が `failed` となって `error` に診断が残り、プロバイダのジョブも失敗になります。どちらの場合も自動では再試行されません。

### ディスパッチ境界の監査

埋め込み同期とリインデックスの enqueue 経路には、Loco の `BackgroundWorker` 拡張点を本番実装として残したまま、キュー障害を決定的にテストするための小さなアプリケーション内ディスパッチ境界があります。
本番アダプターは既存の Loco ワーカー統合点に 1 つだけ置いているため、ワーカー登録、タグ、キューモード、payload、実行処理は変わりません。
REST と MCP のエンティティ書き込みは埋め込み同期の dispatch 前に commit し、リインデックス API も監査行を commit してから dispatch します。
リインデックスの監査 detail は、観測可能な契約変更として意図的に `{ "job_id": ... }` から `{ "workspace_id": ... }` へ変更しました。
キューの `job_id` は dispatch 中に割り当てられるため、この変更により append-only の監査ログと dispatch 前の commit 順序を維持します。
リインデックス API のレスポンスは、ポーリングや運用上の追跡に使うキューの `job_id` を引き続き返します。
スケジューラー、起動時リインデックス検出、ワークスペース埋め込みキー更新は、すでに同じリインデックス enqueue ヘルパーへ委譲するため、別の境界を追加していません。
ワーカー登録、ワーカー実行、キューのルーティング probe は Loco 固有の処理です。ここを置き換えるとアプリケーションの dispatch 方針ではなく、フレームワークの動作をテストまたは置換するため、対象外としました。
Enterprise の infer-fill も同じく狭い境界を使い、dispatch 前に永続ジョブ行を作成し、dispatch 失敗時は failed に更新します。

## Stripe webhook

`YORISHIRO_STRIPE_WEBHOOK_SECRET` を設定すると、Stripe webhook の受信処理が有効になります。
エンドポイントは、JSON を解析する前に、受信した生バイト列が `Stripe-Signature` の HMAC 検証と 300 秒の timestamp 許容範囲を通過したイベントだけを受け付けます。
解析済みイベントは非公開の型付き境界を通って billing 処理へ渡されるため、ビジネスロジックが未検証の payload を受け取ることはありません。
Loco の拡張点はアプリケーションの route や state を構成するためのものであり、プロバイダ固有の受信暗号検証を担当しないため、この境界は Stripe controller の内側に置いています。

## 検索クォータ

| 変数 | 説明 |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | 1分あたりの検索トークン使用量（既定：100000）。クエリごとに1回課金。API と MCP で共有の予算です。既定値は非常に高く、通常の操作では問題になりません。自動クエリの実行を制限するために存在します |

## データベース負荷ガード

PostgreSQL の負荷ガードは、デプロイメント全体のメンテナンス状態を自動変更するため、デフォルトでは無効です。
ステータス値は内部では型付きで扱いますが、API とデータベースの wire 形式では従来の文字列表現を維持します。
負荷が高いと判断するアクティブ接続数を `YORISHIRO_DB_LOAD_THRESHOLD` に設定すると有効になります。
デフォルトでは 5 秒ごとに `pg_stat_activity` を確認し、30 秒間しきい値を超え続けた場合だけ読み取り専用へ移行します。
同じ時間だけ負荷が低い状態に戻ると通常運転へ戻りますが、ガード自身が読み取り専用にした場合だけです。
オペレーターが設定した読み取り専用または完全ロックは、ガードが解除しません。
ガードは PostgreSQL の identity プールを使い、SQLite では何もしません。
これは従来のガード実装を移植したもので、メンテナンス状態の自動変更には明示的な運用判断が必要なため、オプトインの既定値も維持しています。
`cargo loco task db_load_guard` は診断専用で、1 回分の状態を記録するだけでメンテナンス状態は変更しません。

| 変数 | 既定値 | 説明 |
|---|---|---|
| `YORISHIRO_DB_LOAD_THRESHOLD` | 無効（`0`） | 負荷が高いと判断する PostgreSQL のアクティブ接続数 |
| `YORISHIRO_DB_LOAD_SUSTAIN_SECS` | `30` | 負荷が高い、または低い状態を維持する秒数 |
| `YORISHIRO_DB_LOAD_POLL_SECS` | `5` | PostgreSQL の状態を確認する間隔。0 以下の場合は `5` |

## 環境別設定

Yorishiro は Loco 標準の `config/<environment>.yaml` 構成を使います。
`LOCO_ENV` が環境を選び、未設定時は `development` です。
`LOCO_CONFIG_FOLDER` で、選択した YAML を置く別のディレクトリを指定できます。
パッケージ版と Docker 版は `LOCO_ENV=production` を設定し、それぞれの設定ディレクトリにある `production.yaml` を使います。

YAML では Loco の Tera `get_env` 式を使い、`DATABASE_URL`、`QUEUE_URL`、埋め込み設定などのデプロイ時の値を読み込みます。
アプリケーションデータは SQLite と PostgreSQL に対応します。
キューの保存先は Loco のキュープロバイダとして独立して選択します。
このリリースでは SQLite、PostgreSQL、Redis 互換キュープロバイダを利用できます。
development と production の未設定時は両方に SQLite を使うため、外部サービスは不要です。

| 変数 | 既定値 | 説明 |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc` | マルチテナントまたはベクトル検索には `postgres://` を指定 |
| `HOST` | `http://localhost` | サーバのホスト名またはアドレス |
| `YORISHIRO_MAX_TENANTS` | `1` | テナント上限。`0` は無制限となり、初回セットアップウィザードを無効化 |
| `YORISHIRO_MCP_ALLOWED_HOSTS` | 未設定 | `/mcp` で `localhost`、`127.0.0.1`、`::1` に加えて許可する `Host` のカンマ区切りの完全一致値。ワイルドカードは拒否されます |
| `YORISHIRO_QUEUE_KIND` | `DATABASE_URL` から導出 | Loco のキュープロバイダ。このリリースでは `Sqlite`、`Postgres`、`Redis` |
| `QUEUE_URL` | プロバイダ別の既定値 | キュープロバイダの接続 URI。Redis 互換プロバイダは `redis://` または `rediss://` |
| `YORISHIRO_EMBEDDING_BASE_URL` | 未設定 | 埋め込みエンドポイントの URL |
| `YORISHIRO_EMBEDDING_MODEL` | 未設定 | 埋め込みモデル名 |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | 埋め込みプロバイダ（`none` で無効、`local` でローカル、未設定でローカル） |

アプリケーションとキューの両方に SQLite を使う場合、キュー URI は別ファイルを指定する必要があります。
パッケージ版の既定値は `sqlite:///var/lib/yorishiro/yorishiro_queue.sqlite3?mode=rwc`、`config/development.yaml` の既定値は `sqlite://yorishiro_queue.sqlite3?mode=rwc` です。
起動時に SQLite のパスを正規化し、接続クエリを除いて同じファイルになる設定を拒否します。
既存インストールの移行や保存場所の変更には `QUEUE_URL` を明示してください。
PostgreSQL と Redis 互換キューには SQLite の同一ファイル制約はありません。

サポート・CI 検証対象は、SQLite DB と SQLite キュー、PostgreSQL DB と PostgreSQL キュー、および各 DB と Redis 互換 (Valkey) キューです。
SQLite DB と PostgreSQL キュー、または PostgreSQL DB と SQLite キューの交差構成は利用できず、起動前に `ValidationFailed` (`validation_failed`) として拒否します。
エラーメッセージは不正な組み合わせを英日で説明し、DB と同じ種類の SQL キューまたは Valkey の指定を案内します。
キューを指定しない場合は、SQLite DB では SQLite キュー、PostgreSQL DB では PostgreSQL キューを選びます。
RabbitMQ と SQS は未実装で、将来の対応も PostgreSQL DB を使うエンタープライズ版に限り、SQLite DB やコミュニティ版では利用できません。
たとえば PostgreSQL のアプリケーションデータベースと Redis 互換キューサービスを組み合わせる場合は、`YORISHIRO_QUEUE_KIND=Redis` と適切な `QUEUE_URL` を設定します。
今後キュープロバイダが増えても、アプリケーションデータベースの設定は変わりません。

### メール

Yorishiro は既定ではメールを送信しません。
必要な場合は `mailer:` ブロックを追加し、デプロイ時には `MAILER_HOST`、`MAILER_PORT`、`MAILER_USER`、`MAILER_PASSWORD` で上書きします。

### ログ出力

`LOG_LEVEL` 環境変数でログの出力レベルを指定します（`debug`、`info`、`warn`、`error`）。`RUST_LOG` 変数でも設定でき、そちらが優先されます。

## ワーカーとキュー（バックグラウンドジョブ）

ワーカーは、バックグラウンドジョブを取り出して実行する実行プロセスです。
ワーカープールは `WorkerClass` で選ぶ対象グループで、`TenantPrivate`、`Official`、`Shared` のいずれかです。
キューは永続化されたジョブのバックログであり、ワーカープールやデータベース接続プールとは別のものです。
すべての環境設定で `workers.mode: BackgroundQueue` が必須です。
`ForegroundBlocking` と `BackgroundAsync` は設定読み込み時とアプリケーション起動時に拒否されます。

HTTP サーバとワーカーは別プロセスで起動します。
パッケージと Docker Compose は、個人利用向けに shared 対象のワーカーだけを起動します。
ソースからは `cargo loco start --worker=worker-target:shared`、インストール済みバイナリでは `yorishiro start --worker=worker-target:shared` を使用します。
Official または tenant-private の処理には、運用者が追加のプロセスを明示的に起動します。

```console
cargo loco start --worker=worker-target:official
cargo loco start --worker=worker-target:tenant:0198f3a0-0000-7000-8000-00000000000a
```

tenant の ID は小文字・ハイフン付きの正規 UUID とし、workspace の ID は使用しません。
各ジョブには `worker-target:<target>` という複合タグを一つだけ付けます。
Redis では対象タグと同名のキューを対象ごとに使い、ワーカーは指定した対象のキューだけをポーリングします。
検索クエリの埋め込みと Enterprise の infer-fill は shared 対象を使用し、これらだけを選択するワーカー起動方法はありません。
コンマ区切りで複数対象を指定できますが、対象ごとの処理枠は予約されません。
対象なしの `--worker`、`--all`、`--server-and-worker` は起動時に拒否され、通常の `start` は従来どおりサーバーのみを起動します。
Official と tenant の自動起動は運用者がプロセスマネージャーで設定します。
タグ検出用ラッパーや対象ごとの設定ファイルは提供しません。

**旧クラス別ルーティングからのアップグレードでは、まず旧リリースのすべての投入元を停止し、次に旧ワーカーで queued、running、retrying のジョブを処理し終えてから新ワーカーを起動してください。**
アプリケーションの outbox とプロバイダーのキューを処理し終えるまで旧ワーカーと旧 Redis キューを維持し、完了後に旧ワーカーを停止してください。
旧リリースの API やその他の投入元を新ワーカーと並行して動かさないでください。
旧投入元が新ワーカーの起動時検査後に旧形式のジョブを追加すると、そのジョブが処理されずに残ります。
新ワーカーは起動時に outbox に残る旧ルートの未完了 dispatch を検出して拒否しますが、この一度きりの検査では並行する旧形式の投入を防げず、プロバイダーのキュー内の残存ジョブも検査しません。
tenant 対象の Redis キューはジョブの到着に応じて作られ、tenant を削除しても自動削除されません。
キューの増加を監視し、その tenant のジョブを処理し終えた後に不要なキューを削除してください。

各ワーカープロセスには、同じキュー設定と、そのジョブに必要な埋め込みまたは推論の設定が必要です。
ワーカーは HTTP サーバと同じホストでも別サーバでも動かせます。
別サーバのワーカーは、サーバと同じ `QUEUE_URL` を必ず使ってください。
SQLite ファイルを共有キューにする場合は絶対パスを使ってください。
相対パスはホストごとに別々に解決され、共有キューになりません。
PostgreSQL では複数のワーカープロセスが並列にジョブを取り出せます。
SQLite では取り出し処理は直列ですが、永続キューによる障害回復は利用できます。

`YORISHIRO_QUEUE_WORKERS` は 1 ワーカープロセス内の並列取り出しループ数を設定します。
既定値は SQLite キューで 1、PostgreSQL と Redis 互換キューで 2 です。
SQLite は書き込み側が 1 つのため、SQLite キューでワーカーを増やしても競合が増えるだけでスループットは上がらず、起動時に警告を記録します。
`DB_MAX_CONNECTIONS` の既定値は SQLite データベースで 10、PostgreSQL で 100 です。SQLite のプールが 16 を超えると起動時に警告します。
SQL DB とキューの交差構成は、助言ではなく設定検証で拒否します。
SQLite DB と Redis 互換キューは引き続きサポートしますが、他ホストの worker は SQLite ファイルを開けず、名前付きロックもないため、worker を DB と同じホストに置くよう助言します。
スループットは、アプリケーションデータを PostgreSQL、キューを Redis 互換 (Valkey) にした構成が最も伸びます。次に両方 PostgreSQL、最も伸びないのが両方 SQLite です。
`YORISHIRO_QUEUE_REAPER_AGE_MINUTES` は、reaper が処理中ジョブを回復するまでの時間を設定します。

## テナント再インデックススケジュール

テナント単位で定期的にすべてのワークスペースの再インデックスを実行するスケジュールを設定できます。スケジュールは手動の `reindex_embeddings` タスクと同じフローをたどり、ワークスペースごとのプロバイダとモデルバージョンのチェックを尊重します。

API エンドポイント（`POST /api/identity/tenants/schedule` など）で ISO 8601 形式の期間（`P1D` で毎日、`P1W` で毎週）と任意の IANA タイムゾーン名（デフォルト：UTC）を指定します。
間隔には週・日・時・分・秒を使えます(例: `P1D`、`P1W`、`PT6H`)。最短は `PT5M` で、年と月は受け付けません。
`P1D1D` のように単位が重複する指定は受け付けません。
ティッカーは既存の短すぎる間隔を 5 分に補正し、読めない間隔や次回日時がオーバーフローする間隔では 1 日後を使います。
最初の `scheduled_for` はスケジュール設定の 1 間隔後になり、実行した scheduler tick は次の `scheduled_for` をその tick の 1 間隔後に設定します。
各 scheduler tick では、期限を過ぎた `scheduled_for` を実行し、未実行分の猶予時間による打ち切りはありません。

選択した `config/<environment>.yaml` に `scheduler:` エントリを追加して、`TenantReindexScheduler` タスクを固定の cron スケジュールで実行できます。Loco のドキュメントを参照してください。

`cargo loco scheduler` は HTTP サーバとワーカーとは別のプロセスとして実行します。
PostgreSQL では、同じデータベースを使う scheduler replica を複数起動できます。
各 tick は `yorishiro:tenant-reindex-scheduler` の detached なセッション単位 advisory lock を試行し、別 replica が所有している場合は待機せずにスキップします。
PostgreSQL を使うデプロイメントでは、`DATABASE_URL` は PostgreSQL に直接接続するか、session mode の pooler を経由する必要があります。
この所有権 lock はセッション単位であり、スケジュールの選択、更新の commit、すべての queue dispatch の間、同じ DB セッションに保持する必要があるため、transaction mode の pooling はサポートしません。
detached connection はスケジュールの選択、更新の commit、すべての queue dispatch が終わるまで保持します。
その後に明示的に unlock して close します。
接続断やプロセスの異常終了では lock が自動解放されるため、残った replica が次の tick で所有権を取得できます。
所有権トランザクションは期限到来したスケジュールを読み取り、キューへ dispatch する前に次回時刻を更新します。
これは意図した at-most-once dispatch です。
スケジュールの commit 後に scheduler が落ちても重複キューは作られませんが、その commit 後に落ちるかキューが失敗するとその tick の dispatch は失われます。
キューへの dispatch は対象をすべて試行し、1 件でも enqueue に失敗した場合は集約した task failure を返します。
失敗した interval は再試行せず、復旧点は次のスケジュール間隔です。
PostgreSQL では、手動、起動時、定期実行のジョブが重なった場合に、既存のワーカー側ワークスペース単位 advisory lock が実際の再インデックス処理を直列化します。
SQLite では、再インデックスワーカーが単一テナント用のモデル所有経路を直接使い、PostgreSQL の advisory lock は取得しません。

SQLite には PostgreSQL のような advisory lock がないため、scheduler ownership には待機しない OS file lock を使います。
有効な SQLite database path の隣に `.scheduler.lock` suffix の lock file を作り、待機しない OS file lock を取得します。
競合した task process は直ちにスキップし、handle の close またはプロセス終了で lock が解放されます。
この取得は task の実行中に行うため、外部の Loco configuration で scheduler job を指定した場合も対象です。
決定的な隣接 file がないため、in-memory SQLite の scheduler 実行は拒否されます。
`YORISHIRO_SCHEDULER_REPLICAS` は診断・互換性用の metadata にすぎず、実行時の権威としては使いません。
両 backend とも rolling deployment で重複起動できますが、旧 owner が lock を解放してから新しい task が進みます。

## SQLite ANN ベンチマーク

大規模なエンティティ数を持つ SQLite デプロイメントで動作させる場合、フルスキャンのベクトル検索が遅くなる可能性があります。このデプロイメントにはベンチマークタスクが同梱されており、データに対する実際のレイテンシを測定し、vec0 仮想テーブル（KNN インデックス）の採用を推奨するかどうかを判断します。

```
cargo loco task sqlite_ann_benchmark workspace_id:<uuid>
```

タスクは、ベクトル検索クエリの 5 回反復にわたって中央値、最小値、最大値のレイテンシを測定します。エンティティ数が 1000 以上で中央値レイテンシが 200 ms を超える場合、vec0 の採用を推奨します。`min_entities` と `max_latency_ms` CLI 引数で両方の閾値を変更できます。

このタスクは計測専用です。スキーマや構成を変更しません。vec0 仮想テーブルの採用がデプロイメントにとって価値があるかどうかを判断するのに役立ちます。

## ログでの秘密情報の扱い

API キー、webhook secret、OAuth 認証情報、OIDC token、ライセンス所有者情報は debug ログに出力されません。
外部 provider のエラー本文も、認証情報や token が含まれる可能性があるためログに出力しません。
対象には、デプロイメントとワークスペースの embedding または LLM API キー、Stripe webhook secret、OAuth client と state signing の認証情報、OIDC ID token、ライセンス claim の subject が含まれます。
HTTP access log にはリクエスト path だけを記録し、query string は記録しません。そのため OAuth callback の code と state はログに出力されません。

## API キーのライフサイクル

セットアップ後は、テナントの Owner と Admin が `/api/api-keys` でワークスペース API キーを作成、一覧表示、失効できます。
`POST /api/api-keys` は前後の空白を除いた 1 文字以上 100 文字以下の `name` を受け付け、完全なキーをそのレスポンスでのみ返します。
`GET /api/api-keys` は名前、プレフィックス、識別子、作成日時を返しますが、ハッシュや平文の秘密情報は返しません。
`DELETE /api/api-keys/{id}` はキーを直ちに失効させ、識別子が存在しない場合や別テナントのキーの場合は `404` を返します。
リクエストに使用したキー自身の失効は意図的に許可しているため、管理者は別の認証情報なしで古いキーを失効できます。
