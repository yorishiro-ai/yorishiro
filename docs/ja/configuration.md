# 設定リファレンス

[English](../configuration.md) | **日本語**

これは設定の網羅的なリファレンスではない。
埋め込みプロバイダと、ワークスペース単位の検索トークンクォータと、`config/production.yaml`のキュー調整を扱う。
ここに載せる変数はすべて環境変数から直接読まれる。
このブランチにはこれらの設定用の `config.yml` 形式のファイルは無い。

## 埋め込みプロバイダ

`build_embedding_provider`(`src/services/embedding/mod.rs`)が、埋め込みの書き込み(`sync_embedding`)と検索(`GET /api/search`、MCPツールの`search_entities`)の両方で使うプロバイダを選択・設定する。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local`でローカルONNXプロバイダを選択する(後述)。それ以外の値、または未設定の場合はOpenAI互換プロバイダを選択する |
| `YORISHIRO_EMBEDDING_BASE_URL` | OpenAI互換プロバイダのみ。OpenAI互換の埋め込みエンドポイントのベースURL(LM Studio、Ollama、vLLM、または実際のOpenAI)。例: `http://localhost:11434`。`YORISHIRO_EMBEDDING_MODEL`とあわせて設定が必要。どちらか一方でも未設定だと、起動は埋め込みバックエンド未設定のまま進み(失敗しない)、埋め込み呼び出しはすべてリクエスト時に`ProviderUnreachable`で失敗するようになる |
| `YORISHIRO_EMBEDDING_MODEL` | OpenAI互換プロバイダのみ。埋め込みリクエストの`model`フィールドに送られるモデル名。ワークスペース作成時、そのワークスペースが埋め込まれたモデルとしても刻印される。未設定の場合、ワークスペースには`unconfigured`が刻印される |
| `YORISHIRO_EMBEDDING_API_KEY` | OpenAI互換プロバイダのみ。`YORISHIRO_EMBEDDING_BASE_URL`に送るベアラートークン。既定は空文字列で、トークンを確認しないローカルサーバー(LM Studio、Ollama)にはこれが正しい |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | 期待するベクトルの次元数(既定: `768`)。デプロイ内のすべてのベクトルはこの次元数を共有する必要がある。ローカルONNXプロバイダは起動時のプローブ推論でこれを検証し、OpenAI互換プロバイダはレスポンスごとに検証する |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | OpenAI互換プロバイダのみ。`true`にすると埋め込みリクエストに`dimensions`フィールドを含める。既定は`false`で、一部のOpenAI互換実装(vLLM、Ollama、LM Studio)は認識しない`dimensions`フィールドを拒否するため |

### ワークスペース自身の埋め込みプロバイダ(有償版)

`PUT /hosted/workspace/embedding-key`は、どのワークスペースもデプロイ全体で同じ`YORISHIRO_EMBEDDING_BASE_URL`を共有する代わりに、1つのワークスペースの埋め込み処理だけを上記のデプロイ全体共通のものとは別のプロバイダに向ける。
base版には含まれない。
これは既にワークスペース単位でLLM推論の認証情報を割り当てている`PUT /hosted/workspace/llm-key`と同じ切り分けで、どのワークスペースがどの計算先を使うかは有償版の判断である。

| フィールド | 説明 |
|---|---|
| `base_url` | OpenAI互換の埋め込みエンドポイント。例: `https://api.openai.com/v1` |
| `model` | 埋め込みリクエストに送られるモデル名 |
| `api_key` | ベアラートークン。保存はされるが`GET`では返らない。返るのは`base_url`、`model`、`dimensions`、設定済みかどうかのみ |
| `dimensions` | このプロバイダが生成するベクトルの次元数 |
| `send_dimensions_param` | `true`にすると埋め込みリクエストに`dimensions`フィールドを含める。既定は`false`で、前述の`YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM`と同じ |

ここに何も設定していないワークスペースは、引き続きデプロイ全体共通のプロバイダ(`YORISHIRO_EMBEDDING_BASE_URL`など)を使う。
つまり何も設定しなければ、このエンドポイントが存在する前と同じ挙動のままである。
`DELETE /hosted/workspace/embedding-key`で、ワークスペースをそのデプロイ既定値に戻せる。

`PUT`は、ワークスペース自身に刻印されたベクトル幅と一致しない`dimensions`値を、何も保存する前に`422`で拒否する。
これが無いと、ディスク上に既にあるベクトルの幅と合わないプロバイダを割り当ててしまった場合、次にそのワークスペースへエンティティを書き込んだ時点で`sync_embedding`自身の書き込み時チェック(`services/embedding/sync.rs`)に拒否されるまで気づけない。
この2つのチェックはどちらも存在し、どちらも他方を置き換えるものではない。
書き込み時チェックは、ワークスペースがどんな経路で不一致なプロバイダを持つに至ったかに関わらず引き続き効く一方、設定時チェックは運用者がその間違いを犯したまさにその瞬間に同じ問題を表面化させる。

キャッシュは無い。
このエンドポイント経由の設定変更は、そのワークスペースに対して埋め込みプロバイダを解決する次のリクエスト(検索、埋め込み同期)から即座に効く。
何らかの遅延や再起動を待つ必要はない。

### ローカルONNXプロバイダ(`YORISHIRO_EMBEDDING_PROVIDER=local`)

BERT系のONNXモデルをプロセス内で実行し、外部の埋め込みサービスを使わない。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_ONNX_MODEL_PATH` | `.onnx`モデルファイルへのパス。未設定なら初回利用時に自動取得する(後述)。設定した場合はそのパスにファイルが存在している必要があり、無ければこのパスと`YORISHIRO_ONNX_TOKENIZER_PATH`の両方を名指ししたメッセージとともに起動が失敗する |
| `YORISHIRO_ONNX_TOKENIZER_PATH` | トークナイザの`tokenizer.json`へのパス。挙動は`YORISHIRO_ONNX_MODEL_PATH`と同じで、未設定なら自動取得し、設定した場合は存在が必須になる |
| `YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH` | 入力1件あたりの最大トークン数。これを超える分は切り詰められる(既定: `512`) |
| `YORISHIRO_ONNX_POOLING` | `mean`(既定)または`last_token`(`last-token`・`lasttoken`という表記も受け付ける)。未知の値は`mean`へ黙って倒すのではなく起動時点で拒否する。プーリングを間違えたままモデルを読んでも失敗はせず、ただ質の悪いベクトルが返るだけなので、黙って既定値に落としてしまうとその劣化に気づけなくなる |
| `YORISHIRO_ONNX_QUERY_INSTRUCTION` | 検索クエリを埋め込む際にのみ付与する指示文で、保存対象のドキュメント側には一切付かない。クエリ側に指示を要求する非対称なモデル向け(実際には`Instruct: {instruction}\nQuery:{text}`という形でレンダリングされる)。既定は未設定で、この場合は素の`embed`呼び出しとまったく同じになり、対称なモデルにはこれが正しい挙動になる。空文字列も未設定と同じ扱いで、「空の指示を付ける」ではない。変数をクリアするというのは、運用者がこの指示を外すときの操作である |

#### モデルの取得

モデルとトークナイザはリポジトリに入っていない。
モデル単体で約522MiBあるためである。
`YORISHIRO_ONNX_MODEL_PATH`と`YORISHIRO_ONNX_TOKENIZER_PATH`のどちらも未設定で、かつ既定の`models/`にファイルが無い場合、初回利用時に両方を`$HOME/.cache/yorishiro/models/`へ取得し、バイナリに埋め込んだSHA256で検証する。
このディレクトリは次回以降の起動が最初に見る場所でもあるので、ダウンロードの代償を払うのは初回だけである。
`nomic-ai/nomic-embed-text-v1.5`はブランチではなく特定のリビジョンに固定してあり、埋め込んだダイジェストの先にあるバイト列がデプロイの足元で入れ替わることはない。

ダウンロード中は、それを引き起こした処理がその間ブロックする。
直前のログ行がそのことを伝える。
これは多くの場合サーバ起動だが、それだけではない。
`cargo loco task create_workspace`と`cargo loco task resync_embeddings`も埋め込みプロバイダを構築するので、まっさらなマシンで522MiBを引きに行くのがタスク側になることもある。
タスクが止まって見えるときは、まずこれを疑うとよい。

どちらかのパス変数を設定すると、両方のファイルについて自動取得は行われなくなる。
パスを明示した運用者はファイルの在り処を宣言したことになるので、そのパスが間違っていたときは起動を失敗させる。
別の場所へ勝手に半ギガバイトを落としにいくよりそのほうがよい。
既定の`models/`へ手で置く運用もこれまでどおり動き、置いたファイルが上書きされることはない。

失敗の扱いは2種類に分かれる。分ける基準は、起動し直せば直る見込みがあるかどうかである。
ダウンロードに失敗した場合と、取得したバイト列がダイジェストと一致しない場合は、起動を失敗させる。
ネットワーク断は一時的なものなので、再起動するよう設定された監視下ではそれが再試行となり、デプロイは自力で復旧する。
リビジョンを固定したうえでのダイジェスト不一致は破損か改竄であり、検証はまさにそれを止めるために存在する。
一方`HOME`が解決できない場合は取得先そのものが無く、これは再起動しても変わらないので、埋め込みプロバイダ無しで起動し、両方のパス変数を名指ししたメッセージを残す。
この状態では、いずれかの変数を設定するまで検索とリコールはエラーになる。

このプロバイダを組み込んでビルドすると`ort`クレートが入る。
`ort`の既定機能`download-binaries`はビルド時に`cdn.pyke.io`からonnxruntimeバイナリを取得する。
ビルド環境自体を外部から遮断する必要がある場合は、`ORT_LIB_LOCATION`で事前に用意したonnxruntimeを指すこと。

## 検索トークンクォータ

| 変数 | 説明 |
|---|---|
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | ワークスペースが1分間に検索へ使えるトークン数(既定: `100000`)。埋め込みの前、クエリ1回につき1度課金される。`GET /api/search`経由でもMCPツールの`search_entities`経由でも同じで、プロトコルごとではなくワークスペースごとに1つの予算を共有する。予算を超えたクエリは実行されず、HTTP `422`(`validation_failed`)が返る。既定値は通常利用では到達しないほど高く設定されており、通常のトラフィックを制限するためではなく、暴走したエージェントを制限するために存在する |

検索がリクエスト数ではなくトークン数で計測されているのは、それが埋め込みモデルにとっての実際のコストだからである。
エンティティの書き込みはリクエスト数のままで、大きな本文を数えること自体にコストがかかるためである。
クエリのトークン数は`EmbeddingProvider::count_tokens`から得られる。
ローカルONNXプロバイダはこれをトークナイザによる正確なカウントで上書きし、それ以外のプロバイダはすべてバイト長からの概算(`text.len() / 4`、切り上げ)を既定値として使う。
この概算は英語向けに調整されたもので、英語ではおおよそ4バイトで1トークンになる。
日本語のテキストはUTF-8でおおよそ1文字3バイトで、トークナイザにかけるとおおよそ1文字1トークンになるため、同じ日本語クエリに対してこの概算は実際のトークナイザが返す数の半分以下しか返さない。
つまりローカルONNXプロバイダ以外の構成では、日本語の検索クエリは実際のコストより大幅に低く見積もられて予算から差し引かれる。
`YORISHIRO_SEARCH_TOKENS_PER_MINUTE`は、同じ予算値でも英語より日本語の検索トラフィックをかなり多く通してから`422`を返し始めることになる。
デプロイの検索トラフィックが日本語中心で、かつローカルONNXプロバイダを使っていない場合は、この偏りを踏まえて予算値を決めること。

## キューのバックエンドと調整(`config/development.yaml`、`config/production.yaml`)

`queue.kind`は起動時に切り替え可能である。loco-rs は3種のキュープロバイダ(Postgres、SQLite、Redis/Valkey。`QueueConfig`の`#[serde(tag = "kind")]`バリアント)を持ち、それぞれ必要な設定項目が異なる(Redis だけが`queues`を持ち、Postgres/SQLite は SQL プール系の設定を共有しつつ異なる URI を指す)。この違いを1つの固定形で吸収するのは無理があるので、`development.yaml`・`production.yaml`とも`kind`ごとに`queue:`ブロック全体を、Tera の`<% if %>`/`<% elif %>`/`<% endif %>`で丸ごと切り替える形にした。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_QUEUE_KIND` | `Sqlite`(`development.yaml`での既定。同ファイルのデータベース既定値に合わせてあり、未設定のまま起動してもPostgresを必要としない)、`Postgres`、`Redis`のいずれか。`Redis`で起動するには`worker_redis`という Cargo feature のコンパイルが必要(このワークスペースの`Cargo.toml`で有効化済み)。無効のまま起動すると"No queue provider feature was selected and compiled"で失敗する |
| `QUEUE_URL` | キューバックエンド自身の接続URI。`development.yaml`では、両者のバックエンドが一致していれば`DATABASE_URL`が既定値になる。したがって未設定のまま起動すればキューはデータベースと同じSQLiteファイルに入り、PostgreSQLのデプロイならキューも同じPostgreSQLインスタンスに入る。一致しない場合、つまりPostgreSQLの`DATABASE_URL`に対して`YORISHIRO_QUEUE_KIND=Sqlite`を明示した場合は、スキームの異なるURIを渡す代わりにキュー専用のSQLiteファイルにフォールバックする。`production.yaml`はこのファイル自身の「暗黙のフォールバックを許さない」方針どおり、どの`kind`でも既定値なしで必須とする |
| `YORISHIRO_QUEUE_WORKERS` | ジョブを並列に取り出すワーカー数(既定: `2`)。Postgresは`FOR UPDATE SKIP LOCKED`で行を確保するため、この値を上げるとそのバックエンドでは実際に並列度が上がる。SQLite は`BEGIN IMMEDIATE`により、この値に関わらずデキューが直列化される |
| `YORISHIRO_QUEUE_REAPER_AGE_MINUTES` | ジョブが`processing`のまま留まってよい分数で、これを超えるとreaperがそのジョブを`Queued`へ戻す(既定: `30`)。Locoのreaperはopt-inで既定では無効。無効のままだと、実行中に落ちたワーカー(クラッシュ、強制終了)が持っていたジョブは`processing`のまま永久に残る。ほかの何もそのジョブを`processing`から動かさず、`fail_job`は`perform`自体がエラーを返したときにしか走らないためである。健全なジョブが正当にかかりうる最長時間より大きい値を設定すること。そうしないと、reaperはまだ本当に進行中の作業を戻してしまう |

`development.yaml`は`YORISHIRO_QUEUE_WORKERS`/`YORISHIRO_QUEUE_REAPER_AGE_MINUTES`を読む代わりに、同じreaperを固定値(`num_workers: 2`、`age_minutes: 10`)で有効化している。ローカルの開発環境にはデプロイごとに調整する理由が無いためである。`production.yaml`は両方とも読む。
`config/test.yaml`には`queue:`ブロック自体が無く(理由は`.claude/rules/testing.md`を参照)、ここでの説明はいずれも当てはまらない。

`config/sqlite.yaml`(手動確認用のSQLite階層、`docs/ja/sqlite.md`)も`queue: kind: Sqlite`と`workers.mode: BackgroundQueue`を、他の2環境と同じ形で設定している。loco-rsのSQLiteキュープロバイダ(`bgworker::sqlt`)は、アプリ自身のSQLite接続とは独立した`sqlx::SqlitePool`を自前で張る。`db.rs`のRLS対応プール経由ではない(SQLiteにはそもそもRLSが無い)ので、これは同じファイルであれ別のファイルであれ、本当に独立したプールになる。実ファイルに対して直接計測した結果: アプリが同じファイルに対して書き込みトランザクションを開いたままの状態でキュー側から並行して書き込むと、失敗はせず、sqlx自身の既定値である5秒の`busy_timeout`を待った上でロック解放後に成功した。このコードベース自身の実装では、embedding-syncのenqueue呼び出しはリクエスト自身の書き込みトランザクションが既にコミットされた後にしか実行されないため、この状況が1つのリクエストの中で発生することはない。問題になり得るとすれば、最初のリクエストの書き込みトランザクションがまだ開いている間に、別のリクエストが本当に並行して走るケースだけであり、`content_entities::create`自体は1回の`INSERT`で完結する短いトランザクションなので、5秒という猶予には十分な余裕がある。

## サーバとは別プロセス・別ホストでワーカーを動かす

`cargo loco start --worker[=tag1,tag2]`(`yorishiro_core-cli`/`yorishiro_server` どちらでも同じ loco-rs 自身の CLI を共有するので同じ形で使える)は、HTTPサーバを起動せずキューのワーカーループだけを現在のプロセスで動かす。`--worker=worker-class:official` はそのプロセスをそのタグのジョブに限定する(`WorkerClass::tag()`、`src/workers/embedding_sync.rs`)。別ホストの別プロセスであっても、自分の config がサーバと同じ `queue.uri`/`QUEUE_URL` と `database.uri`/`DATABASE_URL` を指してさえいれば足り、追加のネットワーク層や共有シークレット、ノード登録の手順は不要である。

**値なしの `--worker` は、全てのジョブを引き受けるわけではない。** loco-rs 1.1.0 自身のデキューSQL(Postgres・SQLite・Redisいずれのキュープロバイダにも共通する形)を確認した結果、タグを1つも指定しないワーカーが引き受けるのは「タグ無しのジョブ」だけであり、「タグの有無を問わず全部」ではない。このデプロイが積むジョブは必ず`worker-class:*`のタグを1つ持つ(`workers::embedding_sync::enqueue_for_class`)ため、タグ無しのジョブは存在しない。つまり値なしの`--worker`で起動したプロセスは、こうしたジョブを一切デキューしない。「他のプロセスが拾わなかった分を拾う」という動きにはならない。全クラスをカバーする1プロセスを用意したいなら、`--worker=worker-class:tenant-private,worker-class:official,worker-class:shared`のようにタグを全て明示する必要がある。loco-rs 1.1.0にワイルドカードや全件購読の指定方法は無い。

**ワーカー専用プロセスも、キュー接続だけでなくサーバと同じ config 一式を必要とする。** `Hooks::after_context`(src/app.rs)は loco-rs のどの StartMode でも無条件に実行され、これは `--worker` 専用プロセスも例外ではない。そのプロセスが実際にリクエストを処理するかどうかに関わらず、`DATABASE_URL` に対して RLS 対応のテナントプールと migration ロールの identity プールを構築し、embedding provider の設定が誤っていれば起動そのものを失敗させる(「Boot fails loudly ... rather than deferring the error to the first search」というコメント自体が、これが意図的な設計であることを明示している)。どの`WorkerClass`のワーカー型の`perform`も、実際にこの両方を使う。エンティティを再取得するために `ctx.db` を読み、`resolve_embedding_provider` を呼ぶが、これにはサーバと同じ `YORISHIRO_EMBEDDING_*` 環境変数(またはワークスペース自身の割り当て)が必要である。だからキュー接続だけを設定したワーカーノードは、起動した瞬間に落ちる。「ワーカーはキューだけ見ればいい」という思い込みで他の設定を省いてしまう運用者にとっては、ここが一番踏みやすい落とし穴になる。

**どの `WorkerClass` のタグにも購読するプロセスを、タグを明示した形で最低1つ残しておく必要がある。** 稼働中の全ワーカープロセスがタグ限定で、`worker-class:tenant-private`・`worker-class:official`・`worker-class:shared` の全てを名指しするプロセスが1つも無い場合、どのプロセスもカバーしていないクラスのジョブは `pg_loco_queue`/`sqlt_loco_queue` の中に永久に滞留し、誰にもデキューされない。`worker-class:official` 専用ノードを追加するデプロイは、`Shared` および他のどのクラスもカバーしていないプロセスがある場合、3つのタグを全て名指しして動くプロセス(前述のとおり、値なしの `--worker` では代用できない)を最低1つ残す(または追加する)必要がある。

**複数のワーカープロセス・ホストで実際に何が並列化されるかは、キューのバックエンドによって異なる。** これは上記の `YORISHIRO_QUEUE_WORKERS` の行が1プロセス内の `num_workers` について既に説明しているのと同じ区別である。Postgres の `pg_loco_queue` へのデキューは `FOR UPDATE SKIP LOCKED` を使うため、複数のプロセス(1ホストでも複数ホストでも)は実際に異なるジョブを並行してデキューできる。SQLite の `sqlt_loco_queue` へのデキューは `BEGIN IMMEDIATE` を使い、これはファイルの唯一の書き込みロックを取得するため、同じ SQLite ファイルを指す2つ目のプロセスは、最初のプロセスの後ろに直列化される。SQLite バックエンドのキューに対して複数のワーカープロセスを動かすことは、耐障害性(最初のプロセスが落ちた場合に別のプロセスが引き継ぐ)は得られるが、スループットの向上にはならない。

### ワークスペース自身のワーカークラス割り当て(有償版)

`PUT /hosted/workspace/worker-class` は、1つのワークスペースの embedding-syncジョブだけを、共有プールの代わりに `tenant_private` または `official` の計算資源に固定する。
base版には含まれない。どのワークスペースがどの計算先を使うかは、既にワークスペース単位で LLM/embedding の認証情報を割り当てている(`PUT /hosted/workspace/llm-key`、`PUT /hosted/workspace/embedding-key`)のと同じ有償版の判断である。

| フィールド | 説明 |
|---|---|
| `worker_class` | `tenant_private`・`official`・`shared` のいずれか |

ここに何も設定していないワークスペースは、引き続き `shared` のままジョブを処理する。つまり何も設定しなければ、このエンドポイントが存在する前と同じ挙動のままである。
`DELETE /hosted/workspace/worker-class` で、ワークスペースを `shared` に戻せる。
キャッシュは無い。このエンドポイント経由の設定変更は、そのワークスペースに対して次にジョブが積まれた時点から即座に効く。何らかの遅延や再起動を待つ必要はない。
ワークスペースを `tenant_private`/`official` に割り当てても、そのタグを実際に購読するワーカープロセスが動いていなければ、それ自体には何の効果もない(前述の「サーバとは別プロセス・別ホストでワーカーを動かす」参照)。ワークスペースにクラスを割り当てておき、それを実際に処理するノードは後から動かし始める、という順序も成立し、その間ジョブは単に滞留する。
