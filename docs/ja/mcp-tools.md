# MCPツール

以下の一覧は、合成された `rmcp 3.4.0` の実行時ルーターから生成したレビュー用の契約です。
実行時の探索とディスパッチが唯一の正しい情報源であり、この文書を実行時レジストリとして使用してはいけません。
契約テストは、名前、説明、入力スキーマが実行時の生成結果からずれた場合に失敗します。

ツールの名前変更や削除には、マージ前の互換性レビューが必要です。
既存クライアントが古い名前やスキーマに依存する可能性がある場合は、移行期間または非推奨期間を設けてください。
意図的な破壊的変更では、この文書を更新し、変更レビューで移行方針を説明してください。

この一覧は、プロトコルのスコープメタデータを主張しません。
認可は既存の中央集約されたMCP認可経路で引き続き適用され、ツールの説明は人間向けに必要なスコープを説明するだけです。

英語版の生成済みインベントリには、ランタイムから取得した名前、説明、入力スキーマが含まれます。
詳細は [docs/en/mcp-tools.md](../en/mcp-tools.md) を参照してください。

`fill_defaults`はリクエスト内で全件処理せず、設定されたLocoキューへ永続ジョブを投入し、`schema_name`と`job_id`を返します。
RESTの`POST /api/migration-jobs/fill-defaults`も同じ経路を使い、202で受付結果を返します。
完了時の`entities_updated`と`fields_filled`は、migrationスコープの`GET /api/migration-jobs/{job_id}`の`result`から確認できます。
実行前にはエンティティを変更せず、完了後の取り消しには返された`job_id`を使います。

`import_jsonl`は設定されたLocoキューへインポートを投入し、`job_id`を返します。
schemaスコープの`import_job_status`で状態と完了件数を取得してください。
RESTの`POST /api/import.jsonl`も202と`job_id`を返し、`GET /api/import-jobs/{job_id}`で状態を取得できます。
不正な行が含まれる場合はジョブが`failed`となり、それまでのインポートと埋め込みジョブの投入をすべてロールバックします。
インポート本文は永続キューに保存されるため、キューのアクセス権と保存期間もデータ保護の対象です。

`POST /api/migration-jobs/{job_id}/undo`も202で取り消しジョブを受け付け、`job_id`と元の`snapshot_job_id`を返します。
受付時には復元せず、migrationスコープの`GET /api/migration-jobs/{job_id}`で`restored`と`missing`を確認できます。
提案の一括確定は`POST /api/inference-jobs/{job_id}/confirm`で受け付け、schemaスコープの`GET /api/inference-jobs/confirmations/{job_id}`で結果を確認できます。
未完了または失敗した推論ジョブは受付時に409となります。
いずれの状態取得も同じworkspace内の対応する種類のジョブに限り、内部エラーの詳細は公開しません。
