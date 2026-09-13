# 設定

[English](../en/configuration.md) | **日本語**

Yorishiro は環境変数で設定します。設定用のファイルは不要です。

## 埋め込み（自動検索）

Yorishiro はテキストを数値に変換し、類似検索を有効にできます。これを「埋め込み」と呼びます。

### 仕組み

エンティティを作成または更新すると、Yorishiro はバックグラウンドで自動的に埋め込みを生成します。保存が完了した後に実行されるので、書き込みが遅くなることはありません。

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

### ローカルモデル

`YORISHIRO_EMBEDDING_PROVIDER=local` でマシン上でモデルを実行します。

使えるモデルは 1 つだけです：`multilingual-e5-base`（768次元）。日本語を含む多言語に対応しています。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_LOCAL_MODEL` | モデル名（既定：`multilingual-e5-base`）。認識できない値を指定すると起動が失敗する |
| `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` | 入力の最大トークン数（既定：`512`） |

**モデルのダウンロード。** モデルファイルは約 1 GB あり、初めて使うときに自動的にダウンロードされます。2 回目以降はキャッシュされたコピーを使います。`models/<short_id>/` にファイルを手動で置くこともできます。モデルファイルとトークナイザファイルの両方が必要です。

ダウンロードに失敗した場合は再起動すれば再度試行されます。バイナリには組み込みのチェックサムがあり、ダウンロードしたファイルが検証されます。

### 埋め込みモデルの変更

別の埋め込みモデルに変更した場合、既存のエンティティは旧モデルのベクトルのままになります。影響を受けるワークスペースの埋め込み再生成が必要です：

```
cargo loco task reindex_embeddings workspace_id:<uuid>
```

これによりワークスペース内の全エンティティが新しいモデルで再埋め込みされます。リインデックスが完了するまでの間、そのワークスペースへの新規書き込みは拒否されます。

API からもリインデックスを登録できます：`POST /api/migration-jobs/reindex`（Migration スコープが必要）。起動時にワークスペースのモデル変更を自動検知し、リインデックスも走ります。

## 検索クォータ

| 変数 | 説明 |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | 1分あたりの検索トークン使用量（既定：100000）。クエリごとに1回課金。API と MCP で共有の予算です。既定値は非常に高く、通常の操作では問題になりません。自動クエリの実行を制限するために存在します |

## データベース負荷ガード

PostgreSQL の負荷ガードは、デプロイメント全体のメンテナンス状態を自動変更するため、デフォルトでは無効です。
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

## 本番環境の設定（`config/production.yaml`）

設定を何もしなくても SQLite で起動します。外部サービスは不要です。

| 変数 | 既定値 | 説明 |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yotsunagi/yorishiro.sqlite3?mode=rwc` | マルチテナントまたはベクトル検索には `postgres://` を指定 |
| `HOST` | `http://localhost` | サーバのホスト名またはアドレス |
| `YORISHIRO_QUEUE_KIND` | `DATABASE_URL` から自動判定 | キューを分離する場合のみ `Redis` を設定 |
| `QUEUE_URL` | `DATABASE_URL` と共有 | Redis を使う場合のみ必須 |
| `YORISHIRO_EMBEDDING_BASE_URL` | 未設定 | 埋め込みエンドポイントの URL |
| `YORISHIRO_EMBEDDING_MODEL` | 未設定 | 埋め込みモデル名 |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | 埋め込みプロバイダ（`none` で無効、`local` でローカル、未設定でローカル） |

### メール

Yorishiro はメールを送信しません。`MAILER_HOST` を設定した場合だけメール機能が有効になります。

### ログ出力

`LOG_LEVEL` 環境変数でログの出力レベルを指定します（`debug`、`info`、`warn`、`error`）。`RUST_LOG` 変数でも設定でき、そちらが優先されます。

## ワーカー（バックグラウンドジョブ）

Yorishiro は埋め込み生成などの作業をバックグラウンドジョブとして処理します。デフォルトではリクエストを処理するプロセスと同じものがジョブも処理します。

ワーカーを別プロセスで動かす場合：

```
cargo loco start --worker
```

このワーカープロセスにも、メインのサーバと同じ設定（データベース URL、埋め込み設定など）が必要です。

デフォルトでは 2 つのワーカーが並列に動きます。`YORISHIRO_QUEUE_WORKERS` で数を変更できます。クラッシュしたワーカーのジョブを回復するためのリカバリは、既定で 30 分ごとに動作します（`YORISHIRO_QUEUE_REAPER_AGE_MINUTES` で変更可）。

ワーカーは別のマシンでも動かせます。同じデータベースとキュー URL を指すようにするだけです。PostgreSQL を使うと複数のワーカーが並列に動作しますが、SQLite の場合は一度に 1 つずつ処理されます。

## テナント再インデックススケジュール

テナント単位で定期的にすべてのワークスペースの再インデックスを実行するスケジュールを設定できます。スケジュールは手動の `reindex_embeddings` タスクと同じフローをたどり、ワークスペースごとのプロバイダとモデルバージョンのチェックを尊重します。

API エンドポイント（`POST /api/identity/tenants/schedule` など）で ISO 8601 形式の期間（`P1D` で毎日、`P1W` で毎週）と任意の IANA タイムゾーン名（デフォルト：UTC）を指定します。5 分間のグラースウィンドウ内で次の実行を拾う仕組みなので、再起動しても実行が失われることはありません。

`config/*.yaml` に `scheduler:` エントリを追加して、`TenantReindexScheduler` タスクを固定の cron スケジュールで実行できます。Loco のドキュメントを参照してください。

## SQLite ANN ベンチマーク

大規模なエンティティ数を持つ SQLite デプロイメントで動作させる場合、フルスキャンのベクトル検索が遅くなる可能性があります。このデプロイメントにはベンチマークタスクが同梱されており、データに対する実際のレイテンシを測定し、vec0 仮想テーブル（KNN インデックス）の採用を推奨するかどうかを判断します。

```
cargo loco task sqlite_ann_benchmark workspace_id:<uuid>
```

タスクは、ベクトル検索クエリの 5 回反復にわたって中央値、最小値、最大値のレイテンシを測定します。エンティティ数が 1000 以上で中央値レイテンシが 200 ms を超える場合、vec0 の採用を推奨します。`min_entities` と `max_latency_ms` CLI 引数で両方の閾値を変更できます。

このタスクは計測専用です。スキーマや構成を変更しません。vec0 仮想テーブルの採用がデプロイメントにとって価値があるかどうかを判断するのに役立ちます。
