# 環境変数リファレンス

[English](../configuration.md) | **日本語**

`yorishiro-hosted-server`はコミュニティ版一式(`yorishiro-server`)を内包した単一プロセスです。
このリポジトリ独自の変数(下記)に加えて、内包しているコミュニティ版自身の変数(`YSR_EMBEDDING_*`等。[yotsunagi/yorishiroのdocs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)に記載)も読み取ります。
ただし1点例外があります: そのドキュメントに記載されている`config.yml`/`YSR_CONFIG_PATH`によるYAML設定ファイルのサポートは、コミュニティ版バイナリ自身の`main`にのみ配線されており、このプロセスはそれを一切通らないため、`yorishiro-hosted-server`のそばに`config.yml`を置いても何の効果もありません。
すべての設定は実際の環境変数として渡す必要があります。

`YORISHIRO_MAX_TENANTS`は唯一の例外で、このバイナリはコード側で`0`に強制設定するため、環境変数に設定しても効果はありません。

ログ初期化もコミュニティ版の`main`と同じ(`yorishiro_server::logging::init`)ため、`YSR_LOG_TARGET`/`YSR_LOG_DIR`/`YSR_SYSLOG_SOCKET`(stdout/single/daily/syslog。[yotsunagi/yorishiroのdocs/ja/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/ja/configuration.md#ログ出力)参照)がこのバイナリにもそのまま適用されます。
JSON形式でのstdout固定出力ではありません。

DBロードガードも同様に、埋め込みルーターではなく**このバイナリ自身が起動**するため、`YSR_DB_LOAD_THRESHOLD`(既定`0`、無効)・`YSR_DB_LOAD_SUSTAIN_SECS`(既定`30`)・`YSR_DB_LOAD_POLL_SECS`(既定`5`)がここでも有効です。
閾値に正の数を設定しない限り無効のままです。
両エディションで同じ設定にすることが重要で、両者は同一のデータベースを見るため、コミュニティ版のバイナリだけがガードを動かしている構成では、このプロセスが掛けている負荷を監視できません。

| 変数 | 説明 |
|---|---|
| `DATABASE_URL` | PostgreSQL接続文字列(必須)。内包しているコミュニティ版と、このリポジトリ独自のテナント/課金クエリの両方が使います。起動時に`vendor/yorishiro/migrations`(コミュニティ版)、続いてこのリポジトリ独自の`crates/yorishiro-hosted/migrations`(エンタープライズ限定の追加分。OAuthの`identity.users`カラム、Webhook冪等性用の`identity.stripe_processed_events`)が自動適用されます |
| `YORISHIRO_HOSTED_BIND` | リッスンアドレス(デフォルト: `0.0.0.0:8081`)。空文字列(`YORISHIRO_HOSTED_BIND=`)を設定した場合もbind失敗にはならず、同じデフォルトにフォールバックする |
| `YORISHIRO_LICENSE_KEY` | 有償機能を有効化するライセンスキー。RS256で署名したJWTで、バイナリに埋め込んだ公開鍵で検証する。未設定・空・不正・期限切れはいずれも同じ結果になる——有償機能が無効になり、該当エンドポイントは`404`を返す。それ以外は通常どおり動作し、この理由で起動が拒否されることはない。[ライセンスキー](#ライセンスキー)を参照 |
| `YORISHIRO_HOSTED_WEB_DIR` | このリポジトリの管理ダッシュボードSPA(`web/`。`pnpm build`でビルド)を`/`から配信するディレクトリ。Dockerイメージでは`/app/web`(同梱されたビルド成果物)にプリセットされているため、Dockerデプロイでは上書き不要。ベアバイナリデプロイでは`web/`を別途ビルドしてこの変数の設定が必要 — `web/`がバイナリ自体に組み込まれることはない([web-ui.md](web-ui.md)参照)。Docker外で未設定(または空文字列)の場合、`/`はコミュニティ版が組み込んでいるアセットによって配信される |

## OAuth2/OIDCログイン

内包しているコミュニティ版自身のメール/パスワードによる`POST /auth/login`に加えて、追加のオプションのログイン手段です。
これが有効化するエンドポイントについては[api.md](api.md#oauth2oidcログイン)を参照してください。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_OAUTH_ISSUER_URL` | IDプロバイダのissuer URL。例: `https://accounts.google.com`や`https://login.microsoftonline.com/{tenant}/v2.0`。未設定(デフォルト)の場合OAuthログインは完全に無効化され、`/auth/oauth/*`の全ルートが`404 Not Found`を返し、Web UIのログイン画面にもSSOボタンは表示されない(この機能が存在しない過去のデプロイと同じ挙動) |
| `YORISHIRO_OAUTH_CLIENT_ID` | プロバイダに登録したOAuthクライアントID。`YORISHIRO_OAUTH_ISSUER_URL`を設定した場合は必須(未設定または空文字列だと起動時に即座に失敗する) |
| `YORISHIRO_OAUTH_CLIENT_SECRET` | OAuthクライアントシークレット。`YORISHIRO_OAUTH_ISSUER_URL`を設定した場合は必須(未設定または空文字列だと起動時に即座に失敗する)。プロバイダのリダイレクトを経由するCSRF/PKCEの`state`パラメータに署名するHMACキーの導出にも使われる(別途シークレットは不要) |
| `YORISHIRO_OAUTH_REDIRECT_URI` | 認証後にプロバイダがリダイレクトし戻す先。デフォルトは`{YORISHIRO_HOSTED_BIND}/auth/oauth/callback`で、全インターフェースを表すbindホスト(`0.0.0.0`、IPv6の場合は`::`/`[::]`)は`localhost`に書き換えられる(ローカルテストでのみ意味があり、リバースプロキシ経由の公開ホスト名を使う実運用では明示的に設定すべき。ブラウザは`YORISHIRO_HOSTED_BIND`に直接到達できないため) |

OIDCディスカバリドキュメント(`{issuer_url}/.well-known/openid-configuration`)とプロバイダのJWKSは`/auth/oauth/authorize`/`/auth/oauth/callback`の各リクエスト時に都度取得され、起動時にキャッシュされません。
そのため署名鍵やエンドポイントをローテーションするプロバイダでも`yorishiro-hosted-server`の再起動は不要です。

ディスカバリ・JWKS・トークン交換の各リクエストはすべて`https://`必須で、リクエスト途中に`https://`から平文の`http://`にダウングレードするリダイレクトには従いません。
唯一の例外はループバックホスト(`localhost`またはループバックIP)で、TLSを前面に持たないプロバイダ/モックIdPを使ったローカル開発向けに平文`http://`が許可されます——実運用の`YORISHIRO_OAUTH_ISSUER_URL`は常に`https://`にすべきです。

`GET /auth/oauth/authorize`は、ログインフローを開始したブラウザに紐付けるCSRF Cookieを発行します([api.md](api.md#get-authoauthauthorize)参照)。
このCookieの`Secure`属性は`YORISHIRO_OAUTH_REDIRECT_URI`のスキームに従います:`https://`の場合は`Secure`が付与され(`Secure`なCookieは平文HTTPには送信されないため、実運用では必須)、ローカルテスト用のデフォルトである`http://localhost:...`の場合は付与されません。
これを個別に制御する変数はありません——公開の`https://`リダイレクトURIを設定することは、プロバイダがコールバックに到達するために必須である一方、それだけでより厳格なCookie属性も自動的に得られます。

初回のOAuthログイン(このインストールで未見のIDプロバイダ`sub`かつ既存のYorishiroアカウントに一致しない場合)は、新規テナント・ワークスペース・`member`ロールのメンバーシップを自動プロビジョニングします([api.md](api.md#get-authoauthcallback)参照)。
他のテナント作成経路と同様に`YORISHIRO_MAX_TENANTS`の制約を受けますが、前述の通り`yorishiro-hosted-server`は常にこれを無制限に強制設定しています。

`GET /auth/oauth/authorize`/`GET /auth/oauth/callback`は、内包しているコミュニティ版自身の`YSR_AUTH_RATE_LIMIT_MAX`/`YSR_AUTH_RATE_LIMIT_WINDOW_SECS`(デフォルト: クライアントIPごとに60秒あたり10リクエスト——詳細は[yotsunagi/yorishiroのdocs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)参照)によってレート制限され、コミュニティ版自身の`/auth/login`/`/auth/signup`/`/setup*`ルートと同一のクォータを共有します——理由は[api.md](api.md#oauth2oidc-login)参照。
`GET /auth/oauth/status`はレート制限の対象外です。

## Stripe課金

| 変数 | 説明 |
|---|---|
| `YORISHIRO_STRIPE_WEBHOOK_SECRET` | `POST /hosted/stripe/webhook`の`Stripe-Signature`検証に使うStripe Webhook署名シークレット。未設定(デフォルト)の場合、検証不能なリクエストを受け付ける代わりにエンドポイントは`501 Not Implemented`を返す。Stripe課金はopt-in |
| `YORISHIRO_STRIPE_PRICE_PRO` | `pro`プラン(ワークスペース5個、ワークスペースあたりエンティティ50,000件)に対応するStripe Price ID |
| `YORISHIRO_STRIPE_PRICE_TEAM` | `team`プラン(ワークスペース・エンティティともに無制限)に対応するStripe Price ID |

`_PRICE_`系の変数はどちらもデフォルトで未設定(マッピングなし)です。
未知の価格IDを持つ`customer.subscription.*`イベントはログに記録されるだけで適用されません。

Stripeサブスクリプションイベントが一度も適用されていないテナントは`plan = NULL`かつ上限なし、セルフホスト版のテナントと同じ扱いです。
どのStripeイベント種別がどう処理されるかは[api.md](api.md#post-hostedstripewebhook)を参照してください。

## ライセンスキー

有償機能は`YORISHIRO_LICENSE_KEY`に設定するライセンスキーで有効化します。
キーはRS256で署名したJWTで、`sub`(発行先)・`plan`・`exp`(Unixタイムスタンプ)の3つのclaimを持ちます。
検証はバイナリに埋め込んだ公開鍵で行うため、ネットワークアクセスも追加の設定も不要です。

gate対象は4つです: Stripe課金、OAuth2/OIDCログイン、マーケットプレイス(`/api/marketplace/*`)、LLMによる補填(`POST /api/schemas/active/{name}/infer-fill`)。
それ以外——API、MCP、セットアップウィザード、ログイン、メンバー/ワークスペース管理、テンプレートライブラリ——はライセンスキーが無くても動作します。

有効なライセンスが無い場合、gate対象のエンドポイントは`404 Not Found`を返します。
これは提供していない機能に対してこのデプロイが返す答えと同じです。
判定は認証より前に行うため、呼び出し側が有効なAPIキーを持つかどうかで答えは変わりません。

`plan`は記録・ログ出力されますが機能の選択には使いません。
有効かつ期限内のキーであれば4つすべてが解放されます。

マーケットプレイスと補填のgateはリクエストごとに判定するため、サーバ稼働中にキーが期限切れになった場合は再起動を待たずに閉じます。
StripeとOAuthは起動時に設定を読むため、この2つは次回の再起動までそのままです。

起動時に1行、どちらのモードで動いているかを出力します——キーを受理した場合は発行先・プラン・有効期限を、キーが無い場合は有償機能が無効である旨を出します。
設定されているが検証に失敗したキーは警告を出したうえで有償機能を無効にします。
起動そのものは継続します。
有償機能の設定ミスで無償側まで止めることはしないためです。

検証処理は通常のソースコードであり、再ビルドすれば誰でも削除できます。
これは意図的な設計です。
保護するのは`ee/LICENSE`であり、そのようなビルドの利用はライセンス違反として扱います。

## メール

トランザクションメール(招待通知・課金アラート)は現時点で存在しません。
どちらのStripeイベントハンドラからも送信は行われず、実際のプロバイダ(SES/Postmark等)を設定する環境変数もありません。
以前存在した`EmailProvider`トレイトは実装も呼び出し元も無かったため削除済みです。
トランザクションメールを再度追加するには、プロバイダの実装とハンドラへの配線の両方が必要です。
