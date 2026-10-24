# Yorishiro (依り代)

[English](README.md) | **日本語**

[インストール](#始め方) · [初回起動](https://yorishiro-ai.github.io/wiki/ja/installation/first-run/) · [ドキュメント](https://yorishiro-ai.github.io/wiki/ja/) · [リリース](https://github.com/yorishiro-ai/yorishiro/releases)

**AIエージェントから読み書きできる、スキーマを自分で定義するナレッジストア。**

Yorishiroは、構造を持つデータと、そのデータ同士のつながりを保存します。
MCPを通してスキーマを作り、データを書き込み、必要な情報を意味で検索できます。

- **保存する構造を自分で決める。** エンティティの構造をスキーマで定義し、そのスキーマに沿ってデータを管理します。
- **言葉が一致しなくても探す。** キーワードだけでなく意味でエンティティを検索できます。
- **MCPから読み書きする。** スキーマの作成、エンティティの取得・更新、検索をMCPツールとして提供します。[ツール一覧](https://yorishiro-ai.github.io/wiki/ja/reference/mcp-tools/)
- **データ同士のつながりも残す。** エンティティ間にリレーションを作成し、保存した関係を一覧や個別取得で参照できます。
- **テンプレートから始める。** スキーマの構成にはテンプレートも使えます。[テンプレートガイド](https://yorishiro-ai.github.io/wiki/ja/guides/templates/)
- **自分の環境で動かす。** SQLiteは単一テナント向け、PostgreSQLはRLSによるテナント分離に対応しています。[SQLiteの対応範囲](https://yorishiro-ai.github.io/wiki/ja/guides/sqlite/)

---

## 始め方

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 5150:5150 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

起動したサーバーには `http://localhost:5150` で接続できます。
[初回起動の手順](https://yorishiro-ai.github.io/wiki/ja/installation/first-run/)で最初のアカウントと API キーを作成してください。

データを継続して保存するための設定など、運用を始める前に[Dockerガイド](https://yorishiro-ai.github.io/wiki/ja/installation/docker/)を確認してください。
Docker以外では、[パッケージのインストール](https://yorishiro-ai.github.io/wiki/ja/installation/packages/)と[ソースからのビルド](https://yorishiro-ai.github.io/wiki/ja/installation/build/)を案内しています。

## ドキュメント

使い方、設定、運用手順は[Wiki](https://yorishiro-ai.github.io/wiki/ja/)にまとめています。
READMEでは概要と始め方を案内し、詳しい説明はWikiで管理します。

[インストール](https://yorishiro-ai.github.io/wiki/ja/installation/) · [設定](https://yorishiro-ai.github.io/wiki/ja/configuration/) · [埋め込み](https://yorishiro-ai.github.io/wiki/ja/configuration/embedding/) · [ワーカーとキュー](https://yorishiro-ai.github.io/wiki/ja/configuration/workers-and-queues/) · [MCPツール](https://yorishiro-ai.github.io/wiki/ja/reference/mcp-tools/)

## このリポジトリで作業するエージェントへ

このリポジトリで作業する AI エージェントは、まず [AGENTS.md](AGENTS.md)（英語）を読んでください。

## 開発

コードの構成、開発環境、テスト、変更を送る前の確認事項は [CONTRIBUTING.md](CONTRIBUTING.md)（英語）にあります。
依存ライブラリへのローカル修正は [patches/README.md](patches/README.md) で管理しています。

## ライセンス

[Business Source License 1.1](LICENSE) を適用しています。
セルフホスティングは許可されています。
2030-07-14 に、このバージョンのライセンスは GNU General Public License Version 2.0 以降へ自動的に変わります。
