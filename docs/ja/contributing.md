# 貢献ガイドライン

## フレームワークの公開 API

アプリケーション独自の代替実装より、Loco、SeaORM、SeaQuery、Tokio、既存依存の公開 API を優先します。
存在確認には SeaORM の `SelectExt::exists`、行ロックには `lock_exclusive` を使います。
SeaQuery は SQLite が対応しない行ロック句を自動的に省略します。
テストでもキューの生成処理を複製せず、Loco の `create_queue_provider` を使います。
テスト用の代替実装のためだけに本番コードへ trait を追加せず、本番の HTTP クライアントをローカル HTTP サーバーに接続して検証します。
Raw SQL は、RLS のセッション設定、advisory lock、ベクトル操作、条件付きの原子的な書き込みなど、ORM では必要な意味を保てない箇所に限定します。

## 依存のバグ修正

上流依存への一時的な修正は [`patches/`](../../patches/README.md) に置き、ルートの Cargo manifest から参照します。
元バージョン、バグまたは保守停止の問題、変更差分、回帰試験、削除条件を修正ごとに記録します。
同梱した Loco ソースは、`make patch-check` が元アーカイブのハッシュと `fixes.patch` の再適用結果を検証し、ソース全体と比較します。
CI と両方の `check-all` で実行します。
Cargo のキャッシュを書き換えたり、原因を直さず監査除外だけを追加したりしません。

## データベースプール

データベース接続プールは、ワーカープールやキューとは別のものです。
PostgreSQL では、`AppContext.db` は migration role を使う SeaORM プール、`TenantDb` は `ROLE yorishiro_app` を設定する RLS 用の別プールです。
`DbHandle.identity` は `AppContext.db` の接続プールを共有し、追加のプールは作りません。
Loco のキュープロバイダも独立したプールを持ちます。
SQLite には `DbHandle` はなく、`AppContext.db` とキュープロバイダの別 SQLite プールを使います。

本番の `database.max_connections` の既定値は、設定で上書きしない限り両バックエンドで `100` です。
最小接続数の既定値は `1` です。
CI は PostgreSQL の `DB_MAX_CONNECTIONS` を `100` に上書きし、SQLite のテスト設定ではアプリケーションプールの最大値を `10` にしています。
これらの上限は独立したプールごとの値なので、データベース接続の合計は 1 プロセスが作る各プールの合計です。
非同期の待機中はタスクが他の処理へ譲られますが、1 本の物理接続が同時に複数の SQL を実行できるわけではありません。

## テスト

Linux のビルドはリポジトリの `.cargo/config.toml` を使うため、`mold` と `sccache` が必要です。
ローカルで Cargo を実行する前に両方をインストールしてください。

テストスイートは Rust の既定の並列実行で起動してください。
ローカルの一般的なトポロジーには `make test-postgres`、`make test-sqlite`、`make test-valkey` を使います。
`postgres-postgres`、`sqlite-sqlite`、`postgres-valkey`、`sqlite-valkey` の検証には `uv run scripts/test_topology.py <topology>` を使います。
このラッパーは CE または EE の Cargo feature を明示して実行し、環境変数のラベルではなく Loco が解決したテスト設定の証拠を検証します。
テストコマンドが成功してもトポロジーの証拠が不正な場合は、stderr に `YORISHIRO_TOPOLOGY_REJECTED` を出して非 0 で終了します。
CI は両方を確認し、ビルドやテストの失敗を mismatch 自己テストの成功として扱いません。
CI は正式な 4 トポロジーを CE と EE の両方で検証し、版・DB・キューを含む固有名の 8 ジョブを実行します。
`make entities` は CI と同じ `Cargo.lock` に固定された `sea-orm-cli` を要求し、移行だけからの生成を確認するには fresh な PostgreSQL DB を使います。
固定した生成器は部分一意インデックスをエンティティに含めないため、インデックスの所有者は移行だけです。
両方の `check-all` ターゲットにも含まれる `make python-lint` は、CI 分類器とトポロジー自己テストを実行し、各組み合わせが一度ずつ存在することを静的に確認して、交差 SQL 構成や余分な軸を拒否します。
Valkey のトポロジーを専用エンドポイントで検証する場合は `make test-valkey` を使います。
Valkey-compatible な `redis://` エンドポイントにはデータベース 15 を指定してください。Loco はキューの後処理で、このテスト専用データベースを消去します。
リポジトリのその他の補助スクリプトも uv プロジェクトから実行し、Python 標準ライブラリだけを使います。
すべてのテストは `tests/` に置きます。
`src/`、`ee/`、`edition/`、`migration/src/` にテスト属性や `#[cfg(test)]` は置けません。`make test-location-check` がローカルと CI で検出します。
`make debug-macro-check` は Rust コードの `dbg!` と `debug_assert*` を拒否します。常に成立すべき条件には `assert!` を使います。
テストがクレート内の非公開の項目を必要とする場合は、その項目を `pub` に広げ、[public-api.md](public-api.md) の `integration-test` 境界として `scripts/public_api_allowlist.tsv` に登録します。
`tests/` はエンタープライズ機能なしでもビルドできるため、Community エディションを単独でテストできます。`ee/` を必要とするモジュールには `#[cfg(feature = "enterprise")]` を付け、`tests/ee/` は `ee/` の構成を写します。
機能ゲートを置けるのは `tests/` と `edition/` だけです。`src/` はエンタープライズ層の名前を一切出してはならず、`make edition-boundary-check` がこれを検査します。
テスト出力は、プロトコルのハンドシェイク、バックエンドゲートの報告、意図したスキップまたは失敗を説明する診断に限定します。

マイグレーションのテストでは、両方の対応バックエンドについて、初期構築、記録済みの各マイグレーション境界からの段階的なアップグレード、ロールバックを分けて検証します。
PostgreSQL のマイグレーションテストは一意な一時データベースを使い、SQLite のテストは一時ファイルを使うため、各テストが自分のデータベース資源を後始末します。

PostgreSQL の実行例:

```console
$ DATABASE_URL=postgres://yorishiro:yorishiro@localhost:15432/yorishiro \
    DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres \
    make test-postgres
```

SQLite の実行例:

```console
$ DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' \
    LOCO_ENV=test_sqlite make test-sqlite
```

リクエストテストは終了前に `close_app_pools` を呼び出してください。

メタスキーマのバリデーションには、任意のスキーマ形式の定義と空の `entity_types` の拒否を検証するプロパティベーステストが含まれています。

## push 前の確認

```console
$ make check-all
$ make check-all-ee
```

## CI の変更検出

メインの CI ワークフローは `develop` を対象とする pull request でのみ実行します。
push、定期実行、手動起動では実行しません。
CI は明示された base commit との差分を分類し、安定した `Required validation` チェックを報告します。
ドキュメントだけの変更では重いビルドとテストを省略し、未分類のパスがあれば全検証を実行します。
ローカル action の変更では、その action を利用するすべての検証グループを実行します。
パッケージ、Docker、リリース関連の PR では CE/EE のパッケージをビルドし、共通のパッケージ検証を実行します。
パッケージ検証は 8 成果物すべての公開ファイル名と、埋め込まれたパッケージ名、版、release、architecture を照合し、誤名、欠落、版不一致の成果物を拒否します。
fresh install、CE/EE edition 切り替え、同時インストールの拒否、および削除時の管理者設定の動作を検証します。
Docker package ownership 検査は共通 package action から実行し、CI での重複実行を避けます。
リリースワークフローは手動起動専用です。PR からパッケージ、イメージ、タグ、リリースを公開することはありません。
