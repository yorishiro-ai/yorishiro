# SQLite モード

Yorishiro は PostgreSQL の代わりに SQLite で動作できます。ローカルでの評価や、他の人とデータを共有しない単独のワークスペースにご利用ください。

マルチテナントホスティングには SQLite を使わないでください。すべてのデータを単一ファイルに保存し、データベースレベルでのテナント分離がありません。2つ以上のテナントが同じデプロイを共有する場合には PostgreSQL が必要です。

## SQLite で動作するもの

- **エンティティの CRUD**。エンティティの作成、読み取り、更新、削除。
- **ベクトル検索**。埋め込みベクトルによる類似度検索（sqlite-vec）。
- **全文検索**。FTS5 `tokenize='trigram'` による文字n-gramのあいまい検索。
- **API キー認証**。API キーの作成と利用。
- **埋め込み同期**。PostgreSQL と同じ方法でのベクトル生成と保存。
- **スナップショットと巻き戻し**。スナップショットからのエンティティ復元（PostgreSQL で作成されたスナップショットが必要）。

## SQLite で動作しないもの

- **マルチテナント**。SQLite は正確に1テナントのみをサポートします。テナント上限は1にハードコードされており、変更できません。
- **コンテンツフィルタリング**。`filter` クエリパラメータ（JSONB 包含）はエラーを返します。
- **PostgreSQL SQL を必要とするエンタープライズ機能**。`unnest`、`CROSS JOIN LATERAL`、アドバイザリロックに依存する機能は利用できません。
- **SQLite 用スナップショット**。スナップショットの作成はサポートしていません。PostgreSQL で作成されたスナップショットなら SQLite で復元できます。

## 設定

Yorishiro はデータベース URL のスキームで SQLite を検出します。

```sh
export DATABASE_URL='sqlite:///var/lib/yotsunagi/yorishiro.sqlite3?mode=rwc'
cargo run
```

少なくとも2つの接続が必要です。`max_connections` が2未満の場合、サーバーは起動を拒否します。

## 設定ファイル

テスト用の設定が `config/test_sqlite.yaml` にあります（手動検証用）。本番用の SQLite 設定ファイルはありません。`production.yaml` に `sqlite://` URL を設定してください。

## テスト

`make test-sqlite` で SQLite テストを実行します。並列問題を避けるため `--test-threads=1` でインメモリデータベースに対して実行されます。フルテストスイート（`make test`）は PostgreSQL に対してのみ実行されます。

## 認証

認証はテナントプールを経由せずデータベースに直接接続します。行レベルのセキュリティはありませんが、テナントが1人の場合は安全です。
