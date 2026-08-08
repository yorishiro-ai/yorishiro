# 本番デプロイ

[English](../deployment.md) | **日本語**

起動そのものの手順(Docker・ビルド済みバイナリ・ソースから)は[setup.md](setup.md)を参照してください。このガイドはバックグラウンド起動、リリースの切り方、シングルテナント構成をカバーします。

## バックグラウンドで起動する

### Docker

[setup.md](setup.md#dockerで動かす)で使う`-d --restart unless-stopped`は、バックグラウンドで起動し、再起動やクラッシュ後も自動的に立ち上がり直します。

```console
$ docker logs -f yorishiro      # ログ追跡
$ docker stop yorishiro         # graceful shutdown
```

マイグレーションはバイナリに埋め込まれており起動時に自動適用されます(複数レプリカの同時起動もadvisory lockで安全)。SIGTERM/Ctrl-Cでgraceful shutdownし、処理中のリクエストとバックグラウンドのembedding同期の完了(最大30秒)を待ってから終了します。それでもembedding同期が失われた場合は`admin resync-embeddings`で回復できます。

管理CLIは同じイメージで実行できます。

```console
$ docker run --rm -e DATABASE_URL=postgres://... ghcr.io/yotsunagi/yorishiro:latest admin list-tenants
```

未リリースの変更を試すなど、ソースからイメージをビルドしたい場合は、リポジトリ直下の同じマルチステージ`Dockerfile`を使います。

```console
$ docker build -t yorishiro .
```

### systemd(ビルド済みバイナリ)

[setup.md](setup.md#ビルド済みバイナリで動かす)で起動したプロセスを、systemdユニットで再起動をまたいで維持し、異常終了時も自動再起動できます。プレーンなシェルと異なり、systemdの`EnvironmentFile=`は`.env`を直接読み込むため、`source`/`set -a`は不要です。

```ini
# /etc/systemd/system/yorishiro.service
[Unit]
Description=Yorishiro server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro
ExecStart=/opt/yorishiro/yorishiro-server
EnvironmentFile=/opt/yorishiro/.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro
$ journalctl -u yorishiro -f
```

## リリース

リリースは2段階で切ります — バージョンbumpはPR経由で行い(`master`はPRレビューを必須とするrulesetで保護されているため直接pushはできません)、そのPRがマージされるとpublishは自動で走ります。

1. **Bump。** `.github/workflows/release.yml`を`version`入力(例: `0.16.3`、先頭の`v`なし)で実行します。

   ```console
   $ gh workflow run release.yml -f version=X.Y.Z
   ```

   またはGitHubのActionsタブから(`Release`ワークフローを選び「Run workflow」でバージョンを入力)実行することもできます。`prepare`ジョブがバージョン形式を検証し、タグが既存でないか確認した上で、ルート`Cargo.toml`の`workspace.package.version`を書き換え、`cargo update -w`で`Cargo.lock`も追随させ、`release/vX.Y.Z`ブランチをpushして、`github-actions[bot]`名義で`Bump version to vX.Y.Z`というPRを作成します。

2. **ワークフロー実行の承認。** PRの作成者が`github-actions[bot]`であるため、GitHubはそのPRがトリガーするチェック(`check`・`security`など)を、人間がActionsタブでワークフロー実行を承認するまで`action_required`状態のまま保留します。該当の実行を開き、「Review pending deployments」/「Approve and run」(表記は状況により異なります)から承認してください — これを行わないとPRのCIが開始しません。**この手順は見落としやすく、省略するとリリース全体が詰まります。**

3. **レビューとマージ。** CIがすべて成功したら、bump用PRを承認してsquash mergeします。**これだけでpublishまで自動的に走ります** — `release-publish.yml`は`Cargo.toml`を変更する`master`へのpushでも起動するため、bumpのマージがそのままtag作成・ビルド・publishまで連鎖します。

`release-publish.yml`は引き続き`version`入力を与えて手動でも実行できます。

```console
$ gh workflow run release-publish.yml -f version=X.Y.Z
```

これは手順3後の自動実行が発火しなかった場合や途中で失敗した場合のリカバリ手段であり、また自動実行が失敗したバージョンのpublishをやり直す手段でもあります。手動dispatch時は、`prepare`がバージョンを再検証し、タグが既存であれば失敗させ、`master`の現在の`Cargo.toml`が実際に`X.Y.Z`になっているかを確認してから(bump用PRのマージ前に実行してしまった場合や、その後に無関係なコミットが`master`に入った場合を防ぐガードです)`vX.Y.Z`タグを作成・pushします。自動起動(push)時は、バージョンを`master`の`Cargo.toml`から直接読み取り、そのバージョンのタグが既に存在する場合(=依存の更新やバージョン変更を伴わない`Cargo.toml`編集であり、実際のリリースbumpではなかった場合)は、タグ作成もビルドも行わずに正常終了します — これは想定内の動作でエラーではありません。

いずれの経路でもタグ作成に成功すれば、ワークフローの残りの部分が`yorishiro-server`の`x86_64`/`aarch64` Linux(glibc、`linux-amd64`/`linux-arm64`として梱包)と`x86_64` Windows(`windows-amd64.zip`として梱包)バイナリをビルドしてGitHub Releaseに添付し、マルチアーキのDockerイメージを`ghcr.io/yotsunagi/yorishiro:vX.Y.Z`(および`:latest`)としてビルド・pushします。どちらのLinuxアーキテクチャも`ort`/onnxruntimeのビルド要件に合わせて、QEMUを使わずネイティブビルドします。

## シングルテナント構成

`YORISHIRO_MAX_TENANTS=1`・`YSR_EMBEDDING_PROVIDER=local`(いずれも[configuration.md](configuration.md)参照)は共に既定値です。これらを未設定のままにしたデプロイはそのまま[`web/`](../crates/yorishiro-web/web)のSPA(バイナリに組み込み済み)を配信し、そのセットアップウィザード([setup.md](setup.md#初回セットアップ)参照)だけでデプロイの唯一のテナントをオンボードでき、埋め込みにはローカルONNXモデルを使います。テナント上限を外すには`YORISHIRO_MAX_TENANTS=0`を設定してください。
