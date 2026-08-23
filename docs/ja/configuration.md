# 設定リファレンス

[English](../configuration.md) | **日本語**

これは設定の網羅的なリファレンスではない。
埋め込みプロバイダと、ワークスペース単位の検索トークンクォータのみを扱う。
Loco 移植の直近のスライスが触れた2領域である。
ここに載せる変数はすべて環境変数から直接読まれる。
このブランチにはこれらの設定用の `config.yml` 形式のファイルは無い。

## 埋め込みプロバイダ

`build_embedding_provider`(`src/services/embedding/mod.rs`)が、埋め込みの書き込み(`sync_embedding`)と検索(`GET /api/search`、MCPツールの`search_entities`)の両方で使うプロバイダを選択・設定する。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_EMBEDDING_PROVIDER` | `local`でローカルONNXプロバイダを選択する(後述)。それ以外の値、または未設定の場合はOpenAI互換プロバイダを選択する |
| `YORISHIRO_EMBEDDING_BASE_URL` | OpenAI互換プロバイダのみ。OpenAI互換の埋め込みエンドポイントのベースURL(LM Studio、Ollama、vLLM、または実際のOpenAI)。例: `http://10.0.3.200:1234/v1`。`YORISHIRO_EMBEDDING_MODEL`とあわせて設定が必要。どちらか一方でも未設定だと、起動は埋め込みバックエンド未設定のまま進み(失敗しない)、埋め込み呼び出しはすべてリクエスト時に`ProviderUnreachable`で失敗するようになる |
| `YORISHIRO_EMBEDDING_MODEL` | OpenAI互換プロバイダのみ。埋め込みリクエストの`model`フィールドに送られるモデル名。ワークスペース作成時、そのワークスペースが埋め込まれたモデルとしても刻印される。未設定の場合、ワークスペースには`unconfigured`が刻印される |
| `YORISHIRO_EMBEDDING_API_KEY` | OpenAI互換プロバイダのみ。`YORISHIRO_EMBEDDING_BASE_URL`に送るベアラートークン。既定は空文字列で、トークンを確認しないローカルサーバー(LM Studio、Ollama)にはこれが正しい |
| `YORISHIRO_EMBEDDING_DIMENSIONS` | 期待するベクトルの次元数(既定: `768`)。デプロイ内のすべてのベクトルはこの次元数を共有する必要がある。ローカルONNXプロバイダは起動時のプローブ推論でこれを検証し、OpenAI互換プロバイダはレスポンスごとに検証する |
| `YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM` | OpenAI互換プロバイダのみ。`true`にすると埋め込みリクエストに`dimensions`フィールドを含める。既定は`false`で、一部のOpenAI互換実装(vLLM、Ollama、LM Studio)は認識しない`dimensions`フィールドを拒否するため |

### ローカルONNXプロバイダ(`YORISHIRO_EMBEDDING_PROVIDER=local`)

BERT系のONNXモデルをプロセス内で実行し、外部の埋め込みサービスを使わない。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_ONNX_MODEL_PATH` | `.onnx`モデルファイルへのパス(既定: `models/model.onnx`)。リポジトリに同梱されず、自動取得もされない。このファイルまたは`YORISHIRO_ONNX_TOKENIZER_PATH`のいずれかが無いと、両方のパスを名指ししたメッセージとともに起動が失敗する |
| `YORISHIRO_ONNX_TOKENIZER_PATH` | トークナイザの`tokenizer.json`へのパス(既定: `models/tokenizer.json`)。ファイル欠如時の挙動は`YORISHIRO_ONNX_MODEL_PATH`と同じ |

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
