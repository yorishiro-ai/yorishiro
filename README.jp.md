# Yorishiro (依り代)

[English](README.md) | **日本語**

[![codecov](https://codecov.io/gh/yorishiro-ai/yorishiro/graph/badge.svg)](https://codecov.io/gh/yorishiro-ai/yorishiro) [![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/yorishiro-ai/yorishiro)

[インストール](#始め方) · [初回起動](https://yorishiro-ai.github.io/wiki/ja/installation/first-run/) · [ドキュメント](https://yorishiro-ai.github.io/wiki/ja/) · [リリース](https://github.com/yorishiro-ai/yorishiro/releases)

**スキーマを自分で定義できる、AI エージェント向けのナレッジストア。**

[MCP ツール](https://yorishiro-ai.github.io/wiki/ja/reference/mcp-tools/)で、スキーマの定義、エンティティの読み書き、リレーションの管理、意味による検索ができます。
[テンプレート](https://yorishiro-ai.github.io/wiki/ja/guides/templates/)をもとにスキーマを作ることもできます。
SQLite は[単一テナント](https://yorishiro-ai.github.io/wiki/ja/guides/sqlite/)に対応し、PostgreSQL は行レベルセキュリティでテナントを分離します。

---

## 始め方

サーバー、埋め込みワーカー、アプリケーションジョブ用ワーカー、スケジューラーを Docker Compose で起動します。
[Docker Compose ガイド](https://yorishiro-ai.github.io/wiki/ja/installation/docker-compose/)の構成を `compose.yml` に保存し、イメージの `<VERSION>` を利用するリリースタグに置き換えます。
`compose.yml` のあるディレクトリで次のコマンドを実行します。

```bash
docker compose up -d
```

`http://localhost:5150` に接続し、[初回起動の手順](https://yorishiro-ai.github.io/wiki/ja/installation/first-run/)に沿って最初のアカウントと API キーを作成します。

Docker を使わない場合は、[パッケージをインストール](https://yorishiro-ai.github.io/wiki/ja/installation/packages/)するか、[ソースからビルド](https://yorishiro-ai.github.io/wiki/ja/installation/build/)します。

## ドキュメント

[インストール](https://yorishiro-ai.github.io/wiki/ja/installation/) · [設定](https://yorishiro-ai.github.io/wiki/ja/configuration/) · [埋め込み](https://yorishiro-ai.github.io/wiki/ja/configuration/embedding/) · [ワーカーとキュー](https://yorishiro-ai.github.io/wiki/ja/configuration/workers-and-queues/) · [MCP ツール](https://yorishiro-ai.github.io/wiki/ja/reference/mcp-tools/)

## このリポジトリで作業するエージェントへ

このリポジトリで作業する AI エージェントは、まず [AGENTS.md](AGENTS.md)（英語）を読んでください。

## 開発

コードの構成、開発環境、テスト、変更を送る前の確認事項は [CONTRIBUTING.md](CONTRIBUTING.md)（英語）にあります。
依存ライブラリへのローカル修正は [patches/README.md](patches/README.md) で管理しています。

## ライセンス

[Business Source License 1.1](LICENSE) を適用しています。
セルフホスティングは許可されています。
2030-07-14 に、このバージョンのライセンスは GNU General Public License Version 2.0 以降へ自動的に変わります。
