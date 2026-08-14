# Yorishiro Hosted (yorishiro-enterprise)

[English](../../README.md) | **日本語**

このファイルが日本語版のエントリーポイントです。
英語版はリポジトリルートの`README.md`にあります(GitHubの慣習により`docs/README.md`ではなくリポジトリルートに配置)。

[Yorishiro](https://github.com/yotsunagi/yorishiro)のホスティング版(マルチテナント・課金対応)です。
Stripeサブスクリプション課金、プラン/使用量計測、管理ダッシュボードSPAを提供します。
単一プロセス`yorishiro-hosted-server`が、public repoの`yorishiro-server`(コミュニティ版一式。セットアップ・ログイン・ワークスペース管理のWeb UIを含む)をライブラリとして内包し、同じルータに以下のホスティング版限定エンドポイントを追加します。

- `POST /hosted/stripe/webhook` — Stripeサブスクリプション Webhook受信
- `GET /hosted/tenant/overview` — 管理ダッシュボード向けの使用量/プラン概要
- `GET /auth/oauth/authorize` — OAuth2/OIDCログインフローの開始(オプション、`YORISHIRO_OAUTH_ISSUER_URL`の設定が必要)
- `GET /auth/oauth/callback` — OAuth2/OIDCリダイレクトコールバック
- `GET /auth/oauth/status` — Web UI向けOAuthログイン有効性チェック

内包しているコミュニティ版API自体が何を提供するかは、[yotsunagi/yorishiro](https://github.com/yotsunagi/yorishiro)の[docs/api.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/api.md)を参照してください。

## リポジトリ構成

- `crates/yorishiro-hosted` — クレート本体(Stripe Webhook検証、プラン→上限マッピング、使用量計測、管理ダッシュボードREST API)。
  この配下の`migrations/`(下記のvendor分に対するエンタープライズ独自の追加)は`vendor/yorishiro/migrations`と共に起動時に自動適用されます。
  トランザクションメールはまだ存在しません([docs/ja/configuration.md](configuration.md#メール)参照)
- `web/` — エンタープライズ版管理ダッシュボード、rsbuildでビルドするReact SPA。
  バイナリには組み込まれない([docs/ja/web-ui.md](web-ui.md)参照)。
  Dockerイメージにはビルド済みが同梱される。
  ベアバイナリデプロイでは別途ビルドが必要
- `vendor/yorishiro` — public repoのタグ付きリリースにpinされたgit submodule。
  `migrations/`(`sqlx::migrate!`がコンパイル時に解決するパス)用に使います
- `Dockerfile` — マルチステージビルド: SPA(`web/`)、Rustバイナリをそれぞれビルドし、最終イメージに同梱

## 動かし方

ほとんどのデプロイでは、ビルド済みDockerイメージ`ghcr.io/yotsunagi/yorishiro-hosted`か、[GitHub Release](https://github.com/yotsunagi/yorishiro-enterprise/releases)のリリースバイナリをそのまま使うのがおすすめです。
バックグラウンド起動(Dockerの`--restart`/systemd)を含め、[docs/ja/deployment.md](deployment.md)を参照してください。

## ソースからビルド

```console
$ git submodule update --init
$ cargo build --release -p yorishiro-hosted
```

`yorishiro-core`と`yorishiro-server`はpublic repoのタグ付きコミットへのgit依存として取得されるため(`crates/yorishiro-hosted/Cargo.toml`参照)、ビルドにはGitHubへのネットワークアクセスが必要です。

## ドキュメント一覧

| ドキュメント | 内容 |
|---|---|
| [docs/ja/deployment.md](deployment.md) | ビルド済みDockerイメージ/バイナリ、バックグラウンド起動、内包している内容と上書きしている内容、テナントのオンボーディング、リリースの切り方、public repo依存バージョンの更新 |
| [docs/ja/api.md](api.md) | Stripe Webhook、テナント概要、OAuth2/OIDCログインエンドポイントのリファレンス |
| [docs/ja/configuration.md](configuration.md) | 環境変数リファレンス(Stripe・メール・bindアドレス) |
| [docs/ja/web-ui.md](web-ui.md) | 管理ダッシュボードSPA: 画面一覧、スキーマバージョン切り替え |

Stripe関連の設定は開発時には未設定のまま(no-opフォールバック)で構いません。
Stripe課金はデプロイごとのopt-inです([docs/ja/configuration.md](configuration.md)参照)。
メール送信機能は現時点では一切ありません。
