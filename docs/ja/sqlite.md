# SQLite サポート

Yorishiro は評価・シングルテナントでの個人利用のために SQLite 上で動作します。マルチテナントホスティングには位置づけられていません。SQLite にはテナントを分離する行レベルのセキュリティがないためです。

## 制限事項

### マルチテナント分離なし

SQLite は単一ファイルに全データを保存し、データベースレベルでのテナント分離がありません。アプリケーションでフィルタリングを適用していますが、単一のフィルタを見逃すと静かな分離違反になります。2つ以上のテナントをホストするデプロイには PostgreSQL を使ってください。

### ベクトル検索が動作する

ベクトル類似度検索は [sqlite-vec](https://github.com/asg017/sqlite-vec) 拡張機能を使用し、起動時に `sqlite3_auto_extension` を経由でロードされます。`content_entity_embeddings` テーブルはベクトルを生の LE f32 BLOB として保存します（PgVector に相当するものはありません）。KNN 検索は `vec_distance_cosine(ee.embedding, $1)` でコサイン距離を計算するフルテーブルスキャンとして実行され、現在のスケールでは高速です。`content_entities` テーブル自体には `embedding` カラムがありません。全ベクトルクエリは `content_entity_embeddings` 経由でジョインします。

### フルテキスト検索に FTS5 を使用

SQLite には pg_trgm がありません。エンティティの検索で埋め込みがない場合のフォールバックパスとして、FTS5 の仮想テーブル `fts_content_entities` を使用します。マイグレーションで作成され、トリガーで同期されます。FTS5 テーブルには `entity_id UNINDEXED` カラムがあり、ジョインで UUID の ID を使用します（implicit rowid は VACUUM でリネームされる可能性があります）。コンテンツは自動生成の `content` カラムではなく `data` カラムに保存されるため、トリガーは `NEW.rowid` / `OLD.rowid` ではなく `NEW.id` / `OLD.id` を明示的に INSERT します。

### JSONB フィルタリング unavailable

SQLite には `@>` 包含演算子はありません。`filter` クエリパラメータ（JSONB 包含）は SQLite 上で `BackendUnsupported` を返します。

### advisory ロックなし

SQLite では同時に書き込みトランザクションが1つしか許可されないため、advisory ロックは不要です。`db::lock_for_update` は SQLite 上で `Ok(())` を返します。

### ID 生成

PostgreSQL で `uuidv7()` デフォルトの列は、SQLite 上で `ActiveModelBehavior::before_save` によって自動的に ID を生成します。`Entity::insert(...).on_conflict(...)` ビルダーパスを使用するコード（`before_save` をスキップする）は、`db::sqlite_generated_id` で ID を明示的に設定します。

### 埋め込み同期

`content_entity_embeddings` テーブルは両方のバックエンドに存在します。埋め込みの生成と同期は SQLite で同じように動作します。違いは保存形式のみです（BLOB vs. PgVector）。

## 起動時の動作

SQLite 上では、`DbHandle`（PostgreSQL テナントプール）と `Authenticator`  seam は構築されません。認証は直接 `ctx.db` に対して行われます。起動時の `tracing::warn` は PostgreSQL 固有の SQL を使用するエンタープライズ機能のみをリストします（`unnest`、`CROSS JOIN LATERAL`、advisory ロック）。ベクトル検索はこのバックエンドで動作します。

## 設定

`config/sqlite.yaml` は `max_connections: 10` を設定します。SQLite では少なくとも2つの接続が必要です。1つはリクエストトランザクション用、もう1つは独立した `last_used_at` 更新用です。`max_connections < 2` で起動がエラーで失敗し、明確なエラーメッセージが表示されます。

## テスティング

統合テスト（`tests/`）は PostgreSQL でのみ実行されます。SQLite はマニュアル検証専用の環境（`LOCO_ENV=sqlite`）で、テストスイートには接続されていません。`CREATE DATABASE`（テストハーネスで使用する）には SQLite の同等物が存在しないためです。
