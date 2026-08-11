# 運用上の注意

[English](../operations.md) | **日本語**

本プロジェクト自体は以下を自動化する機構を持たないため、運用者側で別途整備してください。

## バックアップ/リストア

データはPostgreSQL(開発環境では`docker-compose.yml`のnamed volume `pgdata`)にのみ保持されます。本リポジトリではComposeのプロジェクト名を明示的に設定していないため、既定ではチェックアウトしたディレクトリ名がボリューム名の接頭辞になります(例: `<ディレクトリ名>_pgdata`)。実際に解決される名前は`docker compose config`で確認してください。本プロジェクトはバックアップの自動化機構を持ちません。

標準的な`pg_dump`/`pg_restore`によるスケジュールバックアップ、またはWALアーカイブ+PITR(Point-in-Time Recovery)構成を運用者側で用意してください。ボリュームのスナップショットのみに頼ると、整合性のないバックアップになる場合があります。

## レート制限

APIキー単位・テナント単位の*レート*制限(リクエストスループットの上限)は現状どこにも実装していません。単一のAPIキーが埋め込み生成や検索を大量に呼び出すと、他のリクエストが遅延しえます。

特に`YSR_EMBEDDING_PROVIDER=local`(ONNXローカル推論)は単一Mutexで推論を直列化しているため、同一テナントはもちろん他テナントの埋め込み生成も待たされます。必要に応じてリバースプロキシ層(nginx/Envoyなど)でAPIキー単位のレート制限を導入してください。

一方、レート制限とは別に、リソース件数のクォータ機構は存在します。テナントの`max_workspaces`とワークスペースの`max_entities`が、それぞれ作成時に強制されます。どちらもデフォルトは`NULL`(無制限)なので、運用者が`admin create-tenant --max-workspaces`/`admin create-workspace --max-entities`で明示的に設定しない限り、セルフホスト運用では一切上限がかかりません。

この機構はテナント/ワークスペースがどこまで大きくなれるかを制限するものであり、リクエストレートを平滑化するものではありません。両者は補完関係にあり、代替はできません。

## 可観測性

embedding同期(entity書き込み後のバックグラウンド処理)の失敗は現状`tracing`ログ(`RUST_LOG`)にのみ出力され、メトリクス基盤には接続していません。

継続的に監視したい場合は、ログ収集基盤(Loki/CloudWatch Logsなど)でのアラート設定に加えて、`admin resync-embeddings`を定期実行し取りこぼしがないか確認することを推奨します。

## アクセスログ

全リクエストはJSON形式のログ行(method・path・status・latency)としてアプリケーションの他の`tracing`出力と同じ形で出力されます。`YSR_LOG_TARGET`で出力先をまとめて切り替えられます。詳細は[configuration.md](configuration.md#ログ出力)を参照してください。

- `stdout`はコンテナランタイムが標準出力からログを収集する運用に向いています。
- `single`/`daily`はログ収集基盤を持たないホスト上でバイナリを直接起動する運用に向いています。
- `syslog`はホストのsyslogデーモンが持つ転送・ローテーション・集約の設定にそのまま乗せる運用に向いています。Linux/Unix系OS限定で、他プラットフォームでは起動時にエラーになります。

`daily`の日次分割を除き、いずれのターゲットも自動でのローテーションや削除は行いません。ディスク使用量を抑えたい場合は`single`/`daily`を`logrotate`等と組み合わせてください。

## 埋め込みモデルを変更する

異なるモデルのベクトルは同じHNSWインデックスに同居できません。また
`YSR_EMBEDDING_DIMENSIONS`が読み込んだモデルと食い違う場合、サーバは**起動を拒否**します。
不一致は黙って検索結果を悪化させるのではなく、プロセスを止めて知らせます。

**既定値の変更は稼働中のデプロイに影響しません。** 次元は環境変数から読むため、
768で動いているデプロイはモデルもベクトルもそのままです。
別のモデルへ移る場合は再埋め込みを行います。

```console
$ # 1. サーバを停止し、models/model.onnx と models/tokenizer.json を差し替える
$ # 2. YSR_EMBEDDING_DIMENSIONS を新しいモデルの次元に設定する
$ # 3. 既存のベクトルを消す(古いモデルのものであるため):
$ psql "$DATABASE_URL" -c "UPDATE content.entities SET embedding = NULL"
$ # 4. サーバを起動し、ワークスペースごとに再生成する:
$ yorishiro-server admin resync-embeddings <workspace-id>
```

手順3と4の間も検索は動きます。埋め込みを持たないエンティティは`pg_trgm`の
フォールバックで到達できるため、この間は結果が悪くなるだけで空にはなりません。

再埋め込みは全データをモデルに通す処理です。クエリではなく一括書き込みと同じ規模で見積もってください。

## メンテナンスモード

2つのモードがあり、いずれもデプロイ全体で共有されます(状態はDBの1行であり、プロセス内の
フラグではありません。フラグだと1台だけメンテナンスに入り、他は書き込みを受け続けます)。

| モード | 読取 | 書込 | ステータス |
|---|---|---|---|
| `read-only` | 許可 | 拒否 | `423 Locked` |
| `full-lock` | 拒否 | 拒否 | `503 Service Unavailable` |

いずれも`Retry-After`を返します。**AIエージェントは本文ではなくヘッダを見て再試行する**ため、
これが無いと即座に再試行され、モードが減らそうとしている負荷をむしろ増やします。

```console
$ yorishiro-server admin maintenance read-only --retry-after 60 --reason "migrating schemas"
$ yorishiro-server admin maintenance-status
$ yorishiro-server admin maintenance off
```

`--reason`は汎用メッセージの代わりに呼び出し元へ表示されます。
「バックアップから復旧中、09:00 復帰予定」と書けば、ステータスコードだけでは生じる
問い合わせを減らせます。

**`/up`と`/health`はどのモードでも応答します。** これらを拒否すると、
オーケストレータが「意図的に止めているサーバ」を再起動してしまい、
再起動しても状態はDBにあるため解消せず、ループが収束しません。

read-onlyの判定はHTTPメソッドで行うため、**`POST /mcp`は読取ツールでも書込として扱われます**。
どのツールかを知るにはリクエストボディを読む必要があり、
ここで消費したボディはハンドラが受け取れなくなるためです。
「書込を通す」より「読取を拒否する」側へ倒しています。

### 監視から切り替える

モードは1行のレコードであり、マイグレーションロールの資格情報を持つものなら何でも
書き込めます。CLIは呼び出し手の1つにすぎません。DB負荷を監視する仕組みがあれば、
書込の遮断を自分で行えます。

```sql
-- 負荷の継続: 書込を止め、いつ戻ればよいかを伝える
UPDATE identity.maintenance
   SET mode = 'read_only', retry_after = 120,
       reason = 'database under sustained load', updated_at = now();

-- 回復
UPDATE identity.maintenance
   SET mode = 'off', reason = NULL, updated_at = now();
```

全リクエストがこの行を読むため、書き込みは次のリクエストから全ノードに効きます。
再起動もデプロイも不要です。

**入りと出は必ず対にしてください。** `read_only`へ切り替えたきり戻さない監視は、
誰かが気づくまで書込を拒否し続けます——夜間の一時的なスパイクが、朝までの障害になります。
オンにする条件が、そのままオフにする条件でなければなりません。

**サーバ自身は負荷を監視しません。** DBのCPU使用率はSQLから取得できないため、
プロセス内で測れるもの——自身のコネクションプール、`pg_stat_database`——は、
しきい値が対象としている負荷ではなく**アプリ側の需要**を測ることになります。

### デフォルト値の補填

`POST /api/schemas/active/{name}/fill-defaults`(schema scope)は、アクティブ版が
`default`を定義しているフィールドを、それ以前に書かれたエンティティへ書き込み、
`job_id`を返します。

**エンティティは自分のスキーマ版のまま**です。値の補填は定義間の移行ではなく、
そのエンティティが元々持ちうるデータを足す操作であり、検証も自分の版で行われます。
どの版に属するかは別の問題です。

**デフォルトを持たない必須フィールドは触れず**、`still_missing`で報告します。
一度書き込んでしまえば、誰も選んでいない値と誰かが選んだ値は区別できません。

`POST /api/migration-jobs/{job_id}/undo`で実行全体を戻せます。スナップショットは
undoで消費されるため**同じjobは1度しか戻せません**——2度目は「その後の変更」の上に
古いデータを被せることになります。

### 値の推測

`fill-defaults`がスキーマから値を読むのに対し、
`POST /api/schemas/active/{name}/infer-fill`(schemaスコープ)は
エンティティが既に持つ内容からモデルに値を提案させます。
対象は「欠けていて、かつ妥当な既定値を持たない」フィールドです——
本文から読み取れる`category`であって、`draft`から始まる`status`ではありません。

**この推論のコストはデプロイメントが負担しません。** 先にワークスペース自身の
資格情報を設定してください:

```console
$ curl -X PUT localhost:8080/api/workspace/llm-key -H "Authorization: Bearer $YSR_KEY" \
    -H 'Content-Type: application/json' \
    -d '{"base_url":"https://api.openai.com/v1","model":"gpt-4o-mini","api_key":"sk-..."}'
```

OpenAI互換のchat-completionsエンドポイントであれば何でも使えます(Ollama・LM Studioを含む)。
`GET`は設定内容の確認のためエンドポイントとモデルを返しますが、**キーは返しません**。
`DELETE`で削除でき、以後`infer-fill`は再び拒否します。

キー未設定のワークスペースは既定値へフォールバックせず**422**を返します——
推測を頼んだ利用者が`default`値を受け取ると、
「推測が行われなかった」ことを知る手段がありません。

**提案は書き込みではありません。** `infer-fill`は`job_id`を返してモデルの提案を保存し、
エンティティには触れません:

```console
$ curl localhost:8080/api/migration-jobs/$JOB_ID/proposals -H "Authorization: Bearer $YSR_KEY"
$ curl -X POST localhost:8080/api/migration-jobs/$JOB_ID/confirm -H "Authorization: Bearer $YSR_KEY"
```

確定すると同じ`job_id`で各エンティティのスナップショットを取ってから適用するため、
`POST /api/migration-jobs/{job_id}/undo`が`fill-defaults`と同じように戻せます。
スキーマが拒否する提案はバッチ全体を失敗させず`skipped`に数えます——
確認済みの残りはそのまま適用されます。

**同じjobは1度しか確定できません。** 適用時に提案を削除するため、
undo後に再度確定してundoが戻した内容を上書きすることはできません。

## キュー基盤の切り替え

`DrainingQueue` は切り替え中に新旧2つのキューを並行させる。**新しい仕事は設置した瞬間から
新キューへ行き**、旧キューは既に受け付けた分を走らせ切る。

1. `DrainingQueue::new(new, old)` を構築してこれで動かす。`Queue` の実装なので**上位は
   切り替え中であることを知らない**
2. `drain_old(timeout)` で旧キューの残件を待ち、**完了したかどうかを返す**。
   `DrainOutcome::TimedOut` は**まだ動いている**という意味であり、手順3へ進んではならない。
   `drain()` ではないのは意図的である——あちらは**新キューに今届いたばかりの仕事まで**
   待ってしまい、切り替えが問うていることではない
3. 破棄して新キューで直接動かす

切り替え経由で送られた仕事が旧キューへ行くことはない。行けば手順2が**動く的を追う**ことになる。
