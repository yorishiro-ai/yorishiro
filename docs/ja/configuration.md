# 設定リファレンス

[English](../configuration.md) | **日本語**

設定は1つのYAMLファイルにまとまっています。
[`config.example.yml`](../../config.example.yml)に全設定の既定値と説明があるので、`config.yml`にコピーして編集してください。
パッケージからの導入では`/etc/yorishiro/config.yml`に置かれ、unitが既にそこを指しています。
それ以外では、作業ディレクトリの`config.yml`、または`YORISHIRO_CONFIG_PATH`が示すパスを読みます。

ファイルが無いこともキーが無いことも、エラーにはなりません。
未知のキーはエラーです。
設定したつもりの値が効かないまま動くより、起動を拒否します。

各設定には環境変数も用意してあり(`config.example.yml`に併記)、環境変数がファイルの値より優先されます。
これによりコンテナ配備や一時的な上書きをファイル編集なしで行えます。
docker composeの`environment:`や`docker compose exec -e`、systemdの`Environment=`、シェルなどで設定してください。
必須ではありません。

## `YSR_`接頭辞は非推奨

変数はすべて`YORISHIRO_*`に統一しています。
旧`YSR_*`名も引き続き受け付けます。
起動時に新しい名前へ写し替え、両方の名前を挙げた警告を出します。
対になるバイナリが無くなった`YORISHIRO_HOSTED_*`名も同様で、`YSR_WEB_DIR`と`YORISHIRO_HOSTED_WEB_DIR`はどちらも`YORISHIRO_WEB_DIR`になります。
新旧の名前を両方設定した場合は新しい方の値を使います。

写し替えは`config.yml`の読み込みより前に行います。
このため、環境変数に設定した旧名は、新しい名前と同じくファイルの値より優先されます。

## config.yml

以下の設定はすべて`config.yml`ファイルでも指定できます。
キー一覧は[`config.example.yml`](../../config.example.yml)を参照してください(`embedding:`・`logging:`・`auth_rate_limit:`はグループごとにネストします)。
デフォルトでは作業ディレクトリの`config.yml`を読み込みます。
別の場所を使う場合は`YORISHIRO_CONFIG_PATH`で指定してください。

ファイルが存在しない場合や、ファイル内に該当キーがない場合はエラーにならず、通常のデフォルト値にフォールバックします。
**環境変数が設定されている場合は、対応する`config.yml`のキーより常に優先されます。**
未知のキー(タイポなど)が含まれている場合は拒否されます — サーバーはそのキーを無視するのではなく、起動に失敗します。

これにより、`config.yml`をデプロイの基本設定として使い、環境変数は一時的な上書き用途(1回限りのDocker `-e`オプションなど)に限定する使い方ができます。

## 基本

| 変数 | 内容 |
|---|---|
| `DATABASE_URL` | PostgreSQL接続文字列(必須) |
| `YORISHIRO_CONFIG_PATH` | 後述の`config.yml`ファイルのパス(既定: 作業ディレクトリの`config.yml`) |
| `YORISHIRO_BIND` | リッスンアドレス(既定: `0.0.0.0:8080`) |
| `YORISHIRO_CORS_ORIGINS` | ブラウザからアクセスする場合の許可オリジン(カンマ区切り。例: 別オリジンで動くダッシュボードが`/auth/login`/`/api/members`を呼べるようにする)。未設定時はクロスオリジン読み取り不可。デバッグビルド限定で、未設定のまま`http://localhost:*`/`http://127.0.0.1:*`(任意ポート)からのアクセスを自動許可する(MCP Inspector等の開発ツール向け)。リリースビルドではこの自動許可は無効 |
| `YORISHIRO_MAX_TENANTS` | `admin create-tenant`が作成できるテナント数の上限。未設定時は既定で`1`(シングルテナント)。無制限にするには`0`を、複数許可するにはその上限数を設定する。`POST /auth/signup`はテナントを作成しない(既存テナントへ招待を引き換えるだけ)ため影響を受けない。初回セットアップウィザード([setup.md](setup.md#初回セットアップ)参照)もこの変数で有効/無効が決まり、上限が`0`でない場合のみ有効化される |
| `YORISHIRO_WEB_DIR` | Web UIは`ee/web/dist`からバイナリに組み込まれ、既定で`/`から配信される。実ディレクトリから配信させたい場合に設定する。リクエストごとに読み直すため、バイナリを再ビルドせずUIを編集・反映できる |
| `YORISHIRO_AUTH_RATE_LIMIT_MAX` / `YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS` | `/auth/signup`・`/auth/login`・`/setup`(bearerトークン不要なエンドポイントであり、未認証の呼び出し元が総当たりできる唯一の経路)に対する、呼び出し元IPごとのレート制限。既定値: 60秒あたり10リクエスト |
| `YORISHIRO_SEARCH_TOKENS_PER_MINUTE` | 1ワークスペースが1分間に検索へ使えるトークン数(既定: `100000`)。**検索だけをトークンで計量する**——それが埋め込みモデルへの実コストであり、書き込みは本文が大きく計量自体が書き込みより高くつくためリクエスト数のままとする。予算を超える単発クエリも1回は通り、その後ウィンドウを使い切った状態になる |
| `YORISHIRO_SNAPSHOT_RETENTION_DAYS` | 一括移行を取り消せる日数(既定: `30`。`0`以下で無期限保持)。移行は触れたエンティティ1件につき変更前イメージを1行書き、取り消し以外では消えない——無制限にすると、移行を繰り返すワークスペースでイメージが実データを上回る。掃除はタイマーではなく、そのワークスペースで次に移行が走った時点で行う。期限を過ぎたジョブの取り消しは`404`——一度も実行されなかったジョブと同じ答えになる。32bit整数にならない値は丸めず既定値に戻す——600万年の保持は打ち間違いであり、最も近い有効値を採ればそれを隠してしまう |
| `RUST_LOG` | ログレベル(例: `info`) |

## DBロードガード

データベースの負荷が続いている間だけデプロイを読み取り専用に落とし、負荷が引いたら戻します。
閾値を設定しない限り無効です。
求められてもいないのに読み取り専用へ落とすのは既定の挙動として重すぎ、また適切な値は`max_connections`に依存しますが、それはサーバが決める値ではないためです。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_DB_LOAD_THRESHOLD` | この接続数を超えると読み取り専用になる。未設定または`0`でガード自体が無効 |
| `YORISHIRO_DB_LOAD_SUSTAIN_SECS` | 閾値超過が何秒続いたら切り替えるか(既定: `30`)。瞬間的なスパイクで切り替わらないようにする |
| `YORISHIRO_DB_LOAD_POLL_SECS` | 接続数を確認する間隔(既定: `5`) |

## リクエスト相関

すべてのレスポンスに`x-request-id`ヘッダが付与される。
リクエストに既に付いていればその値をそのまま返し、無ければサーバがUUIDを生成する。
同じ値がそのリクエストのtracingスパンにも付くため、処理中に出た`warn`/`error`(認証拒否・レート制限超過・内部エラー等)はアクセスログの行と同じ`request_id`フィールドを持つ。
障害調査の際、特定の失敗したリクエストとサーバ側のログ行を突き合わせるのに使える。

拒否されたリクエスト(APIキー不正・欠落、スコープ不足、レート制限超過)は呼び出し元IPとパスを添えて`warn`でログ出力される(提示されたキー自体は出力しない)。

## ログ出力

HTTPアクセスログ(method・path・status・latency)を含む全てのログ行はJSON形式で出力されます。
`YORISHIRO_LOG_TARGET`で出力先を選択できます。

| 変数 | 内容 |
|---|---|
| `YORISHIRO_LOG_TARGET` | `stdout`(既定、コンテナランタイムのログドライバ向け)、`single`(単一ファイルへ追記、ローテーションなし)、`daily`(日次ローテーションするファイル)、`syslog`(Linux/Unix系OS限定。他プラットフォームでは起動時にエラーになる) |

### `YORISHIRO_LOG_TARGET=single`または`daily`の場合

| 変数 | 内容 |
|---|---|
| `YORISHIRO_LOG_DIR` | ログファイルの出力先ディレクトリ(既定: `.`)。ファイル名は`yorishiro.log`固定で、`daily`の場合は日付が付与される(例: `yorishiro.log.2026-07-13`) |

### `YORISHIRO_LOG_TARGET=syslog`の場合

| 変数 | 内容 |
|---|---|
| `YORISHIRO_SYSLOG_SOCKET` | RFC 3164形式のメッセージを送信するUnixドメインソケット(既定: `/dev/log`)。Linux/Unix系OS限定 |

## 埋め込みプロバイダ

| 変数 | 内容 |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local`(既定)または`openai` |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | 埋め込みベクトルの次元数(既定: `1024`。既定モデルの出力次元)。使用するモデルの出力次元と一致する必要があります。**ワークスペースは作成時にこの値を記録し、異なるモデルによる書き込みは拒否されます**(下記) |

### `YORISHIRO_EMBEDDING_PROVIDER=local`の場合(ONNXエクスポート、既定)

| 変数 | 内容 |
|---|---|
| `YORISHIRO_ONNX_MODEL_PATH` | ONNXモデルのパス(既定: `models/model.onnx`) |
| `YORISHIRO_ONNX_TOKENIZER_PATH` | tokenizerのパス(既定: `models/tokenizer.json`) |
| `YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH` | 最大シーケンス長(既定: `512`) |
| `YORISHIRO_ONNX_POOLING` | トークン埋め込みを1本のベクトルへ集約する方式: `mean`(既定)または`last_token`。**好みではなくモデルの性質**であり、sentence-transformers系(bge-small・multilingual-e5・all-mpnet)は`mean`、Qwen3-Embedding系は`last_token`を要求する。誤った方式で読んでもエラーにはならず検索品質だけが落ちるため、未知の値は起動失敗とし既定へフォールバックしない |
| `YORISHIRO_ONNX_QUERY_INSTRUCTION` | 検索クエリにのみ前置する指示文。Qwen3-Embedding系は`Instruct: {task}\nQuery:{text}`を要求する。**保存する文書には付かない**。未設定または空文字列で無効(既定)。対称なモデルでは設定しない |

### 埋め込みモデルを変更する

ワークスペースは作成時のモデルと次元数を記録します。
異なる次元のベクトルを書き込もうとすると、両方の数値を示して**422**で拒否されます。

このチェックが無いと書き込み自体は成功し(列は次元を持たないため)、
そのワークスペースの次の検索が `different vector dimensions 384 and 1024`で失敗します。
**原因となったエンティティも書き込みも示されません。**

別のモデルへ移す場合は、デプロイメントをそのモデルに向けたうえで再埋め込みします:

```console
$ yorishiro-server admin resync-embeddings --workspace <id>
```

この記録が導入される前に作られたワークスペースは記録を持たず、
デプロイメントが生成するものをそのまま受け入れます(従来どおりの挙動)。

### `YORISHIRO_EMBEDDING_PROVIDER=openai`の場合(例: Ollama, LM Studio, OpenAI)

| 変数 | 内容 |
|---|---|
| `YORISHIRO_EMBEDDING_BASE_URL` | `/v1/embeddings`互換エンドポイントのベースURL(必須) |
| `YORISHIRO_EMBEDDING_MODEL` | モデル名(必須) |
| `YORISHIRO_EMBEDDING_API_KEY` | エンドポイントが要求する場合のAPIキー |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | リクエストボディに`dimensions`パラメータを含めるか。未設定時は既定で`true`。一度設定すると、小文字の文字列`true`と完全一致する場合のみ有効のまま — `false`・`False`・`FALSE`・`0`等それ以外の値はすべて無効(`false`)として扱われる |

具体的な取得例(`https://huggingface.co/Xenova/multilingual-e5-large`の`onnx/model_quantized.onnx`と`tokenizer.json`)は[docs/ja/embedding-providers.md](embedding-providers.md)を参照してください。
