# キュー開始目標

Yorishiro は、アプリケーションのワーカーアダプターから投入したジョブごとにライフサイクル行を記録します。

## 意味

`enqueue_at` は、プロバイダーへ投入する前にアプリケーションが記録する時刻です。

`claim_at` と `start_at` は、ワーカーハンドラーがジョブを受け取った最初の時刻です。
Loco はプロバイダーの claim フックを公開していないため、これらは意図的に同じ値であり、観測可能な claim と開始の境界を表します。

キュー開始レイテンシーは `start_at - enqueue_at` です。
プロバイダーの再試行や再配送でも最初の投入時刻を保持するため、再試行によって開始が速く見えることはありません。

ライフサイクルの状態は `queued`、`running`、`retrying`、`completed`、`failed`、`cancelled`、`unavailable` です。
キュー容量の不足とプロバイダーエラーは、開始成功として数えません。
重複配送は、ライフサイクル行が実行中または終端状態なら無視されます。
実行中の行には5分間の lease があります。
lease が有効な再配送は `Duplicate` とし、lease の期限切れ後の再配送は行を回収して `attempt` を増やします。
終端遷移は受け入れ時の attempt で保護するため、古いワーカーが回収後の attempt を完了または再試行に変更することはありません。

## 容量と診断

各ジョブには複合対象タグを一つだけ指定します。`worker-target:shared`、`worker-target:official`、または `worker-target:tenant:<tenant-id>` です。
ライフサイクル行には、クラス、任意のプラン、同時実行キー、設定された上限を時刻情報とともに保存します。
`attempt` は、実行中の処理として受け入れた配送時だけ増加します。

PostgreSQL と SQLite では、ライフサイクルをアプリケーションデータベースに永続化します。
Redis はプロバイダー側のジョブ状態を提供しますが、Redis の検査方法はプロバイダー固有であるため、ライフサイクルの永続性はアプリケーションデータベースに置きます。
運用ダッシュボードでは、プロバイダーの queued、processing、completed、failed、cancelled 状態とライフサイクル行を組み合わせてください。

ライフサイクル行のコミット後に主エンティティへの書き込みが失敗した場合、その投入失敗はライフサイクルの診断情報と構造化ログに記録されます。
すでにコミット済みのライフサイクル行へロールバックして呼び出し元に返すことはできません。
キュープロバイダーが未設定または到達不能の場合、呼び出し側は行を `unavailable` として扱い、プロバイダーエラーを返します。

## 運用

対象タグを明示してワーカーを起動してください。
対象なしの起動は拒否されます。
開始時には、既存の構造化 tracing に `queue_start_seconds` と `lifecycle_id` が出力されます。
デプロイ先のログ・可観測性基盤で、このイベントをエクスポートまたは絞り込んでください。
この Issue では別のメトリクスエンドポイントは追加しません。
容量による延期は `worker capacity saturated`、再試行への遷移は `retrying`、プロバイダーまたはポリシーの失敗は diagnostic フィールド付きの `unavailable` として出力されます。
ライフサイクル行を `worker_class` と `status` で集計し、記録された開始時刻をプランの目標と比較してください。
`queued`、`retrying`、または `unavailable` が増え続ける場合、SLA を判断する前に容量またはプロバイダーの健全性を復旧してください。

この Issue では、プリエンプション、バーストクレジット、RabbitMQ、SQS、提供計算、WASM 実行は追加しません。

## 優先度と容量

Loco 1.2.0が提供する検証済みのスケジューリング機能を使用します。対応providerは整数priorityを受け付けます。

PostgreSQLとSQLiteは`ORDER BY priority DESC, run_at, id`で処理対象のジョブを選ぶため、そのqueryから見える行の順序は決定的です。

Redisはpriority順のZSETの先頭1000件だけをscanし、その後tagの一致を確認してpriority、`run_at`、idで候補を並べます。

workerに一致しない先行エントリがある場合、tag付きRedis jobがこの上限を超えて次のpolling cycleまでskipされることがあります。

Yorishiroは`tenant_private`に300、`official`に200、`shared`に100のpriorityを割り当てます。

ライフサイクルの admission lock は `shared:shared`、`official:official`、`tenant_private:tenant:<tenant-id>` の対象キー単位で実行数を数えます。
plan はライフサイクルのメタデータとして残り、キーには入りません。

JWTのplan claim `enterprise` はTeamの上限にマッピングされます。

capacity が埋まったジョブは対象タグを保持したまま retrying として記録し、クラスの priority で再投入します。
別の対象へ暗黙に変更しません。

処理枠を予約するには、対象ごとにワーカープロセスを起動します。
`--worker=worker-target:shared`、`--worker=worker-target:official`、`--worker=worker-target:tenant:<tenant-id>` を使用します。

複数対象をまとめたワーカーも利用できますが、`num_workers` は共有 polling pool であり、対象ごとの処理枠は予約しません。

providerの挙動は固定しているLoco 1.2.0のソースで確認し、全providerに同じordering保証があるとは扱わずproviderごとに記載しています。

starvation protectionは、providerのqueued rowを後からreprioritizeするのではなく、durableなadmission boundaryで行います。

低いクラスのライフサイクルが1分間`queued`または`retrying`で待機した場合、そのdurable rowを観測した新しい高いクラスのdispatchには、低いクラスのworkがadmitされるまでpriority 50を割り当てます。

これにより閾値到達後の連続到着から保護しますが、provider内ですでに先行しているジョブはproviderのorderingとbounded scanの挙動に従います。

この抑制は候補ワーカーを共有する新規投入にのみ適用されます。
対象別ワーカープロセスは処理枠を分離します。

scheduling decision、クラス、priority、capacity limit、active capacity、saturation、fallbackは構造化tracing fieldとして出力します。
