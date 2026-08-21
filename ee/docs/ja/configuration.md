# 環境変数リファレンス

[English](../configuration.md) | **日本語**

以下は`ee/`の有償機能が読み取る変数です。
それ以外が読み取る変数は[設定リファレンス本体](../../../docs/ja/configuration.md)にあり、単一のプロセスが両方を読みます。

`config.yml`と`YORISHIRO_CONFIG_PATH`もここで有効です。
バイナリは1つであり、そのバイナリ自身がファイルを読み込みます。

変数はすべて`YORISHIRO_*`です。

| 変数 | 説明 |
|---|---|
| `DATABASE_URL` | PostgreSQL接続文字列(必須)。サーバ全体が共有します。有償機能のテーブルも含む単一の`migrations/`ディレクトリが起動時に自動適用されます |
| `YORISHIRO_BIND` | リッスンアドレス(デフォルト: `0.0.0.0:8080`)。空文字列(`YORISHIRO_BIND=`)を設定した場合もbind失敗にはならず、同じデフォルトにフォールバックする |
| `YORISHIRO_LICENSE_KEY` | 有償機能を有効化するライセンスキー。RS256で署名したJWTで、バイナリに埋め込んだ公開鍵で検証する。未設定・空・不正・期限切れは起動時点ではいずれも同じ結果になる: 有償機能が無効になり、それ以外は通常どおり動作する。この理由で起動が拒否されることはない。無効化された機能が何を返すかは機能ごとに異なる。[ライセンスキー](#ライセンスキー)を参照 |
| `YORISHIRO_WEB_DIR` | 管理ダッシュボードSPA(`ee/web/dist`。`pnpm build`でビルド)は`rust-embed`でバイナリに組み込まれており、この変数は未設定であればそれをそのまま`/`から配信する。設定した場合はディスク上のそのディレクトリを毎リクエスト読み直して代わりに配信する(ビルド済みSPAを再ビルドなしに差し替えたいときのオプトインの上書き) |

## OAuth2/OIDCログイン

組み込みのメール/パスワードによる`POST /auth/login`に加えて、追加のオプションのログイン手段です。
これが有効化するエンドポイントについては[api.md](api.md#oauth2oidcログイン)を参照してください。

| 変数 | 説明 |
|---|---|
| `YORISHIRO_OAUTH_ISSUER_URL` | IDプロバイダのissuer URL。例: `https://accounts.google.com`や`https://login.microsoftonline.com/{tenant}/v2.0`。未設定(デフォルト)の場合OAuthログインは完全に無効化され、`/auth/oauth/*`の全ルートが`404 Not Found`を返し、Web UIのログイン画面にもSSOボタンは表示されない(この機能が存在しない過去のデプロイと同じ挙動) |
| `YORISHIRO_OAUTH_CLIENT_ID` | プロバイダに登録したOAuthクライアントID。`YORISHIRO_OAUTH_ISSUER_URL`を設定した場合は必須(未設定または空文字列だと起動時に即座に失敗する) |
| `YORISHIRO_OAUTH_CLIENT_SECRET` | OAuthクライアントシークレット。`YORISHIRO_OAUTH_ISSUER_URL`を設定した場合は必須(未設定または空文字列だと起動時に即座に失敗する)。プロバイダのリダイレクトを経由するCSRF/PKCEの`state`パラメータに署名するHMACキーの導出にも使われる(別途シークレットは不要) |
| `YORISHIRO_OAUTH_REDIRECT_URI` | 認証後にプロバイダがリダイレクトし戻す先。デフォルトは`{YORISHIRO_BIND}/auth/oauth/callback`で、全インターフェースを表すbindホスト(`0.0.0.0`、IPv6の場合は`::`/`[::]`)は`localhost`に書き換えられる(ローカルテストでのみ意味があり、リバースプロキシ経由の公開ホスト名を使う実運用では明示的に設定すべき。ブラウザは`YORISHIRO_BIND`に直接到達できないため) |

OIDCディスカバリドキュメント(`{issuer_url}/.well-known/openid-configuration`)とプロバイダのJWKSは`/auth/oauth/authorize`/`/auth/oauth/callback`の各リクエスト時に都度取得され、起動時にキャッシュされません。
そのため署名鍵やエンドポイントをローテーションするプロバイダでも`yorishiro-server`の再起動は不要です。

ディスカバリ・JWKS・トークン交換の各リクエストはすべて`https://`必須で、リクエスト途中に`https://`から平文の`http://`にダウングレードするリダイレクトには従いません。
唯一の例外はループバックホスト(`localhost`またはループバックIP)で、TLSを前面に持たないプロバイダ/モックIdPを使ったローカル開発向けに平文`http://`が許可されます。
実運用の`YORISHIRO_OAUTH_ISSUER_URL`は常に`https://`にすべきです。

`GET /auth/oauth/authorize`は、ログインフローを開始したブラウザに紐付けるCSRF Cookieを発行します([api.md](api.md#get-authoauthauthorize)参照)。
このCookieの`Secure`属性は`YORISHIRO_OAUTH_REDIRECT_URI`のスキームに従います:`https://`の場合は`Secure`が付与され(`Secure`なCookieは平文HTTPには送信されないため、実運用では必須)、ローカルテスト用のデフォルトである`http://localhost:...`の場合は付与されません。
これを個別に制御する変数はありません: 公開の`https://`リダイレクトURIを設定することは、プロバイダがコールバックに到達するために必須である一方、それだけでより厳格なCookie属性も自動的に得られます。

初回のOAuthログイン(このインストールで未見のIDプロバイダ`sub`かつ既存のYorishiroアカウントに一致しない場合)は、新規テナント・ワークスペース・`member`ロールのメンバーシップを自動プロビジョニングします([api.md](api.md#get-authoauthcallback)参照)。
他のテナント作成経路と同様に`YORISHIRO_MAX_TENANTS`の制約を受けるので、デフォルトの上限`1`のセルフホスト運用では、2つ目のテナントはプロビジョニングされず拒否されます。

`GET /auth/oauth/authorize`/`GET /auth/oauth/callback`は`YORISHIRO_AUTH_RATE_LIMIT_MAX`/`YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS`(デフォルト: クライアントIPごとに60秒あたり10リクエスト。詳細は[設定リファレンス本体](../../../docs/ja/configuration.md)参照)によってレート制限され、`/auth/login`/`/auth/signup`/`/setup*`と同一のクォータを共有します。
理由は[api.md](api.md#oauth2oidcログイン)参照。
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
それ以外(API、MCP、セットアップウィザード、ログイン、メンバー/ワークスペース管理、テンプレートライブラリ)はライセンスキーが無くても動作します。

マーケットプレイスと補填はリクエストごとにgateします。
有効なライセンスが無い場合は`404 Not Found`を返します。
提供していない機能に対してこのデプロイが返す答えと同じです。
判定は認証より前に行うため、呼び出し側が有効なAPIキーを持つかどうかで答えは変わりません。
リクエストごとに判定するため、サーバ稼働中にキーが期限切れになればこの2つは再起動を待たずに閉じます。

StripeとOAuthのgateは仕組みが異なります。
ライセンスが無いプロセスはそもそもこの2つを構成しないため、変数を一度も設定していないデプロイとまったく同じ挙動になります: `/hosted/stripe/webhook`は`501 Not Implemented`を、`/auth/oauth/*`の各ルートは`404`を返します。
これは起動時に一度決まるため、稼働中にキーが期限切れになってもこの2つは次回の再起動まで有効なままです。

`plan`は記録・ログ出力されますが機能の選択には使いません。
有効かつ期限内のキーであれば4つすべてが解放されます。

キーの発行先(`sub`)はログに出しません。
自由記述であり、メールアドレスが入ることが普通にあるためです。

起動時に1行、どちらのモードで動いているかを出力します: キーを受理した場合はプランと有効期限を、キーが無い場合は有償機能が無効である旨を出します。
設定されているが検証に失敗したキーは警告を出したうえで有償機能を無効にします。
起動そのものは継続します。
有償機能の設定ミスで無償側まで止めることはしないためです。

検証処理は通常のソースコードであり、再ビルドすれば誰でも削除できます。
これは意図的な設計です。
保護するのは`ee/LICENSE`であり、そのようなビルドの利用はライセンス違反として扱います。

## メール

トランザクションメール(招待通知・課金アラート)は現時点で存在しません。
Stripe Webhookの処理経路からも送信は行われず、実際のプロバイダ(SES/Postmark等)を設定する環境変数もありません。
以前存在した`EmailProvider`トレイトは実装も呼び出し元も無かったため削除済みです。
トランザクションメールを再度追加するには、プロバイダの実装とハンドラへの配線の両方が必要です。
