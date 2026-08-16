# 本番デプロイ

[English](../deployment.md) | **日本語**

起動そのものの手順(Docker・ビルド済みバイナリ・ソースから)は[setup.md](setup.md)を参照してください。
このガイドはバックグラウンド起動、リリースの切り方、シングルテナント構成をカバーします。

## バックグラウンドで起動する

### Docker

[setup.md](setup.md#dockerで動かす)で使う`-d --restart unless-stopped`は、バックグラウンドで起動し、再起動やクラッシュ後も自動的に立ち上がり直します。

```console
$ docker logs -f yorishiro      # ログ追跡
$ docker stop yorishiro         # graceful shutdown
```

マイグレーションはバイナリに埋め込まれており起動時に自動適用されます(複数レプリカの同時起動もadvisory lockで安全)。
SIGTERM/Ctrl-Cでgraceful shutdownし、処理中のリクエストとバックグラウンドのembedding同期の完了(最大30秒)を待ってから終了します。
それでもembedding同期が失われた場合は`admin resync-embeddings`で回復できます。

管理CLIは同じイメージで実行できます。

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

未リリースの変更を試すなど、ソースからイメージをビルドしたい場合は、リポジトリ直下の同じマルチステージ`Dockerfile`を使います。

```console
$ docker build -t yorishiro .
```

### systemd(パッケージを使わない場合)

`.deb`と`.rpm`は自前のunitを同梱し`systemctl enable --now yorishiro`で有効化するため、この節は[パッケージの外に取り出した](setup.md#パッケージの外でバイナリを動かす)バイナリを別の場所で動かす場合のものです。
パッケージ同梱のunitと同じく、`YORISHIRO_CONFIG_PATH`で設定ファイルを指します。
このunit名はパッケージのものではないため任意に決められます。

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
Environment=YORISHIRO_CONFIG_PATH=/opt/yorishiro/config.yml
Restart=on-failure
# 78 は EX_CONFIG。サーバは「設定が無い/使えない」場合にのみこの値で終了する。
# これが無いとデータベース未設定の起動が5秒ごとに永久再試行し、`systemctl is-failed` は
# `failed` ではなく `activating` を返すため、unit の状態を見る監視からは障害が見えない。
# それ以外の失敗は従来どおり再試行される(起動途中のデータベースはそれで復帰する)。
RestartPreventExitStatus=78
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro
$ journalctl -u yorishiro -f
```

## リリース

リリースはワークフローの起動1回で完結します。
`.github/workflows/release.yml`が、バージョン更新・タグ作成・全成果物のビルド・公開イメージの起動確認・GitHub Releaseの作成までを行います。

```console
$ gh workflow run release.yml -f version=X.Y.Z
```

Actionsタブからも実行できます(`Release`ワークフローを選択 →「Run workflow」→ 先頭の`v`を除いたバージョンを入力)。
**`master`から起動する必要があります** — チェックアウトした内容をそのままpushするため、他のrefからの起動はワークフロー側で拒否します。

実行内容は以下の順です。

1. バージョンが先頭ゼロなしの`x.y.z`形式か検証し、新規リリースか再開かを判定します(後述)。
2. ルート`Cargo.toml`の`workspace.package.version`を更新し、`cargo update -w`を実行した上で、bumpコミットと`vX.Y.Z`タグを**まとめて(atomicに)**`master`へpushします。
3. 両エディションを`x86_64`と`aarch64`のLinux向けにビルドし、それぞれ`.deb`と`.rpm`に梱包します(計8ファイル)。
   どちらのLinuxアーキテクチャも`ort`/onnxruntimeのビルド要件に合わせQEMUを使わずネイティブビルドします。
4. マルチアーキのDockerイメージを`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`および`:latest`としてビルド・pushします。
5. **公開したイメージを実際にpullし、本物のPostgreSQLに対して起動**します。
   `/up`が応答しなければリリースを失敗させます。
6. GitHub Releaseを作成し、8つのパッケージとそれらを対象とした`checksums.txt`を添付します。
   添付前に各グループの個数を数え、1つでも空なら公開せず失敗します(何にもマッチしないglobはアップロードアクションにとってエラーにならないため)。

### 失敗したリリースからの復旧

手順2でタグはbumpコミットとまとめて確定するため、手順3〜5で失敗すると**タグはあるがGitHub Releaseが無い**状態になります。
この場合は**同じバージョンでもう一度実行してください。** ワークフローはタグの有無ではなく**GitHub Releaseの有無**で状態を判定します。

| 状態 | 実行時の挙動 |
|---|---|
| タグなし | 通常のリリース(bump → タグ → 公開) |
| タグあり・Releaseなし | 再開。bumpを飛ばし、既存タグから公開をやり直す |
| タグあり・Releaseあり | 明示的に失敗。そのバージョンは公開済み |

GitHub Releaseは全成果物のpushとスモークテスト通過の**後**に作成されるため、「このバージョンは出荷済み」の目印として信頼できます。
リモートタグを手で削除したり、パッチ番号を1つ飛ばしたりする必要はありません。

## シングルテナント構成

`YORISHIRO_MAX_TENANTS=1`・`YORISHIRO_EMBEDDING_PROVIDER=local`(いずれも[configuration.md](configuration.md)参照)は共に既定値です。
これらを未設定のままにしたデプロイはそのまま[SPA](../../ee/web)(バイナリに組み込み済み)を配信し、そのセットアップウィザード([setup.md](setup.md#初回セットアップ)参照)だけでデプロイの唯一のテナントをオンボードでき、埋め込みにはローカルONNXモデルを使います。
テナント上限を外すには`YORISHIRO_MAX_TENANTS=0`を設定してください。
