# ホスティング版のデプロイ

[English](../deployment.md) | **日本語**

`yorishiro-hosted-server`は単一のプロセス/バイナリです。
public repoである[yotsunagi/yorishiro](https://github.com/yotsunagi/yorishiro)のコミュニティ版(`yorishiro-server`)一式(スキーマ/エンティティ/検索/認証すべて)をライブラリとして内包し、そこにこのリポジトリ独自のルート(Stripe課金・使用量計測・管理ダッシュボードSPA)を同じルータへマージしています。
これと一緒に起動するものは他になく、ホスティング版のデプロイで`yorishiro-server`単体が別プロセスとして動くことはありません。

ホスティング(マルチテナント)には合わないコミュニティ版のデフォルト(シングルテナント上限、初回セットアップウィザード)は、運用者が環境変数の設定を忘れる余地を残さないよう、このバイナリのコード側で強制的に上書きしています。
設定可能な項目は[configuration.md](configuration.md)を参照してください。

このリポジトリが[yotsunagi/yorishiroのdocs/api.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/api.md)の上に何を追加しているかは[api.md](api.md)を参照してください。

以下2つの起動方法から1つを選んでください。

## Dockerで動かす

リリースのたびに`ghcr.io/yotsunagi/yorishiro-hosted:vX.Y.Z`(および`:latest`)が公開されます(詳細は下の[リリースの切り方](#リリースの切り方)参照)。
Dockerイメージはこのリポジトリの管理ダッシュボードSPA(`web/`)を専用のNodeステージでビルドし、`/app/web`に同梱、`YORISHIRO_HOSTED_WEB_DIR`もプリセットするため、エンタープライズ版ダッシュボードはそのまま配信されます。

`web-builder`ステージの`node:24-slim`は、現行のActive LTSであるNode 24(Krypton)を追随しています。
Nodeのメジャーバージョンは偶数(24、26、…)がLTSになり、奇数(25、27、…)は決してLTSにならないという交互のパターンを持ちますが、Dependabotの`docker`エコシステムでは「偶数メジャーのみ」という条件を表現できないため、`.github/dependabot.yml`では`node`のメジャーバージョン更新を一律で無視しています(minor/patchは引き続き自動で提案されます)。
次の偶数LTS(26)への移行は、他の意図的なバージョン更新と同様に`Dockerfile`/`web/package.json`(`engines.node`)を手動で更新する必要があり、Dependabotが自動的に提案してくることはありません。

1. このリポジトリは非公開なのでGHCRパッケージも非公開です。
   `read:packages`スコープとこのリポジトリへのアクセス権を持つPATで`docker login ghcr.io`しておきます。

   ```console
   $ echo "$GITHUB_TOKEN" | docker login ghcr.io -u <github-username> --password-stdin
   ```

2. コンテナを起動します。

   ```console
   $ docker run -d --name yorishiro-hosted --restart unless-stopped -p 8081:8081 \
       -e DATABASE_URL=postgres://... \
       -e YORISHIRO_STRIPE_WEBHOOK_SECRET=... \
       -e YORISHIRO_STRIPE_PRICE_PRO=... -e YORISHIRO_STRIPE_PRICE_TEAM=... \
       -e YSR_EMBEDDING_PROVIDER=openai \
       -e YSR_EMBEDDING_BASE_URL=https://api.openai.com/v1 \
       -e YSR_EMBEDDING_MODEL=text-embedding-3-small \
       ghcr.io/yotsunagi/yorishiro-hosted:latest
   ```

   Embedding providerの指定、またはローカル用ONNXモデルファイルの用意が必要です。
   デフォルトの`local`はこのイメージに同梱されていないONNXモデルファイル(`models/model.onnx`、`models/tokenizer.json`)を読み込もうとするため、明示的にプロバイダを指定しないとリスナーをbindする前に起動が失敗します。
   `openai`(エンドポイントとモデルを指定する`YSR_EMBEDDING_BASE_URL`/`YSR_EMBEDDING_MODEL`とセットで使用)を使うか、モデルディレクトリをマウントして`YSR_ONNX_MODEL_PATH`/`YSR_ONNX_TOKENIZER_PATH`で指定してください。
   embedding provider関連の全変数は[yotsunagi/yorishiroのdocs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)を参照してください。

3. 起動を確認します。

   ```console
   $ curl localhost:8081/up
   ```

`-d --restart unless-stopped`はバックグラウンドで起動し、再起動やクラッシュ後も自動的に立ち上がり直します。
`docker logs -f yorishiro-hosted`でログ追跡、`docker stop yorishiro-hosted`でgraceful shutdownできます。
`DATABASE_URL`はこのプロセス自身の接続先です。
別プロセスの`yorishiro-server`と共有する構成ではありません。

マイグレーション(public repoから`vendor/yorishiro/migrations`として取り込んだもの、続いてこのリポジトリ独自の`crates/yorishiro-hosted/migrations`)は起動時に自動適用され、advisory lockにより複数レプリカからの同時起動も安全です。
環境変数の全リストは[configuration.md](configuration.md)を参照してください(内包しているコミュニティ版自身の設定、例えば`YSR_EMBEDDING_PROVIDER`もこのバイナリが読み取ります。任意のSSOログインを有効化する`YORISHIRO_OAUTH_*`変数も含みます)。

未リリースの変更を試すなど、ソースからイメージをビルドしたい場合:

```console
$ git submodule update --init
$ docker build -f Dockerfile -t yorishiro-hosted .
$ docker run --rm -p 8081:8081 \
    -e DATABASE_URL=postgres://... \
    -e YORISHIRO_STRIPE_WEBHOOK_SECRET=... \
    -e YORISHIRO_STRIPE_PRICE_PRO=... -e YORISHIRO_STRIPE_PRICE_TEAM=... \
    -e YSR_EMBEDDING_PROVIDER=openai \
    -e YSR_EMBEDDING_BASE_URL=https://api.openai.com/v1 \
    -e YSR_EMBEDDING_MODEL=text-embedding-3-small \
    yorishiro-hosted
```

## ビルド済みバイナリで動かす

Dockerを使わない、ベアメタル/VMへのデプロイ向けです。

1. 自分のアーキテクチャ向けのリリースアーカイブを取得して展開します。

   ```console
   $ mkdir -p /opt/yorishiro-hosted && cd /opt/yorishiro-hosted
   $ curl -L -o yorishiro-hosted.tar.gz \
       https://github.com/yotsunagi/yorishiro-enterprise/releases/download/vX.Y.Z/yorishiro-hosted-server-vX.Y.Z-linux-amd64.tar.gz
   $ tar -xzf yorishiro-hosted.tar.gz && rm yorishiro-hosted.tar.gz
   ```

   このアーカイブには`yorishiro-hosted-server`バイナリそのものしか含まれていません。
   このリポジトリの管理ダッシュボード`web/`は**同梱されていません** — 配信したい場合は別途ビルドし(`web/`で`pnpm build`)、その出力を`YORISHIRO_HOSTED_WEB_DIR`で指定してください([web-ui.md](web-ui.md)参照)。
   指定しない場合、`/`はコミュニティ版が組み込んでいるアセットによって代わりに配信されます。
2. `DATABASE_URL`など[configuration.md](configuration.md)の各変数を1行1つの`KEY=value`形式で環境ファイルに書きます。

   ```console
   $ cat > yorishiro-hosted.env <<'EOF'
   DATABASE_URL=postgres://...
   YORISHIRO_STRIPE_WEBHOOK_SECRET=...
   YORISHIRO_STRIPE_PRICE_PRO=...
   YORISHIRO_STRIPE_PRICE_TEAM=...
   YSR_EMBEDDING_PROVIDER=openai
   YSR_EMBEDDING_BASE_URL=https://api.openai.com/v1
   YSR_EMBEDDING_MODEL=text-embedding-3-small
   EOF
   ```

   `YSR_EMBEDDING_PROVIDER`のデフォルト`local`は、このバイナリに同梱されていないONNXモデルファイルを必要とします。
   詳細は上の[Dockerで動かす](#dockerで動かす)の注記を参照してください。
   ここでも同じ制約が適用されます。

3. 読み込んで起動します。

   ```console
   $ set -a; source yorishiro-hosted.env; set +a
   $ ./yorishiro-hosted-server
   ```

systemdで再起動をまたいで動かし続ける方法は[バックグラウンドで起動する](#バックグラウンドで起動する)を参照してください。

## バックグラウンドで起動する

ベアメタル/VMへのデプロイでは、systemdユニットを使うと[ビルド済みバイナリで動かす](#ビルド済みバイナリで動かす)のプロセスを再起動をまたいで維持でき、異常終了時も自動再起動されます。
プレーンなシェルと異なり、systemdの`EnvironmentFile=`は環境ファイルを直接読み込むため、`source`/`set -a`は不要です。

以下のユニットはrootではなく専用の`yorishiro`システムユーザーで動作します。
先にユーザーを作成し、`/opt/yorishiro-hosted`の所有権を渡してください(Dockerイメージも同様に`useradd --system --no-create-home yorishiro` / `chown -R yorishiro:yorishiro`を行っています。`Dockerfile`参照):

```console
$ sudo useradd --system --no-create-home yorishiro
$ sudo chown -R yorishiro:yorishiro /opt/yorishiro-hosted
```

```ini
# /etc/systemd/system/yorishiro-hosted.service
[Unit]
Description=Yorishiro Hosted server
After=network.target

[Service]
WorkingDirectory=/opt/yorishiro-hosted
ExecStart=/opt/yorishiro-hosted/yorishiro-hosted-server
EnvironmentFile=/opt/yorishiro-hosted/yorishiro-hosted.env
Restart=on-failure
User=yorishiro

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now yorishiro-hosted
$ journalctl -u yorishiro-hosted -f
```

## 内包している内容と上書きしている内容

`yorishiro-hosted-server`は`yorishiro-server`自身の`main`が呼ぶのと同じ`build_app`/`build_embedding_provider`関数を呼んでいるため、コミュニティ版のREST/MCP/検索/認証の機能一式はpublic repoに記載の通りそのまま動作します(`YSR_EMBEDDING_PROVIDER`などコミュニティ版自身の環境変数も含め、詳細は[yotsunagi/yorishiroのdocs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)を参照)。

ただし、ホスティング(マルチテナント)デプロイがコミュニティ版のセルフホスト向けデフォルトのまま正しく動くことは決してないため、いくつかの項目は運用者設定に任せずコード側で固定しています。

- `YORISHIRO_MAX_TENANTS`は`main`の一番先頭で`0`(無制限)に強制設定されます。
  環境変数からは読み取らないため、ホスティング版がシングルテナント上限のまま誤って起動することはありません。
- テナント上限が無制限になる結果、コミュニティ版の初回セットアップウィザード(`GET /setup/status` / `POST /setup`)は自動的に無効化されます。
  ここでのテナントは常にStripeのチェックアウトか招待の消費によって作成され、ウィザードは使いません。
- このバイナリは`YSR_WEB_DIR`を一切読みません。
  代わりに自身の`YORISHIRO_HOSTED_WEB_DIR`を`build_app`に渡しており、これは`yorishiro-server`自身の`main`で`YSR_WEB_DIR`が制御しているのと全く同じフォールバックを制御します。
  未設定ならコミュニティ版自身が組み込んでいるアセット(セルフホスト版のコミュニティデプロイが配信するのと同じもの)にフォールバックし、設定すれば実ディレクトリで上書きします。
  このリポジトリ独自の`web/`(エンタープライズ版管理ダッシュボード)はバイナリ自体にはコンパイルされて組み込まれませんが(rsbuildで別途ビルドするプロジェクト)、Dockerイメージでは`/app/web`にビルド済みが同梱され`YORISHIRO_HOSTED_WEB_DIR`もプリセットされます。
  ベアバイナリデプロイでは別途ビルドして変数の設定が必要です。
  詳細は[web-ui.md](web-ui.md)を参照してください。

## 管理CLI

エンタープライズ版バイナリにはコミュニティ版と同じ管理サブコマンドが含まれています。
以下のように実行します。

```console
$ ./yorishiro-hosted-server admin <command>
```

利用可能なコマンド: `create-tenant`, `list-tenants`, `create-workspace`, `list-workspaces`, `create-user`, `add-member`, `list-members`, `create-invite`, `create-api-key`, `list-api-keys`, `revoke-api-key`, `resync-embeddings`

管理コマンド実行時、vendor(コミュニティ版)とlocal(エンタープライズ専用)の両方のマイグレーションが自動適用されます。
`set_ignore_missing(true)`により、各マイグレーションランナーは自身が管理していないマイグレーションIDを無視します。

詳細は `./yorishiro-hosted-server admin --help`を参照してください。

## テナントのオンボーディング

テナント作成と初回ownerアカウントの発行は、public repoに記載の手順(管理CLI、または招待を消費する`POST /auth/signup`。[yotsunagi/yorishiroのdocs/setup.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/setup.md#signup-login-member-and-workspace-management)参照)とまったく同じです。

`YORISHIRO_OAUTH_ISSUER_URL`を設定している場合([configuration.md](configuration.md#oauth2oidcログイン)参照)、テナントは自己オンボーディングも可能です。
組織から最初にSSOでサインインした人に対し、新規テナント・ワークスペース・`member`ロールのメンバーシップが自動的にプロビジョニングされ、招待は不要です。
2人目以降のチームメイトは、パスワード方式のサインアップと同様に最初のメンバーからの招待が必要です。
自動プロビジョニングが発動するのは、このデプロイで未見のIDプロバイダ`sub`に対してのみであり、SSOログインのたびに発動するわけではありません。

テナントはStripeがサブスクリプションを報告するまで`plan`も`max_workspaces`上限も持ちません(`checkout.session.completed`でStripeカスタマーを紐付け、続く`customer.subscription.created`/`updated`でプランを適用)。
詳細は[api.md](api.md#post-hostedstripewebhook)を参照してください。

## リリースの切り方

リリースはActionsタブから`.github/workflows/release.yml`を手動でdispatchして切ります(`gh workflow run release.yml -f version=X.Y.Z`でも可)。
バージョン番号は先頭の`v`を付けずに渡します(例: `0.12.2`)。
`workflow_dispatch`が解決するのは`master`上にある`release.yml`自体の内容のみです(そのためワークフローファイル自体への変更はdispatch可能になる前にmasterへマージされている必要があります)が、Actionsタブでは実行「対象」のブランチ/タグも選択でき、その選択が実際に効くのは最初の`prepare`ジョブのcheckoutのみです。
ワークフローの最初のステップは`master`以外に対する`prepare`の実行を拒否するため、誤ったブランチ/タグを選んでも意図しない内容がリリース・pushされる前に安全に失敗します。
`prepare`が成功した後、後続のバイナリ/Docker/GitHub Releaseの各ジョブは、最初に選択したrefではなく`prepare`が新たに作成した`v<version>`タグをcheckout・ビルド対象とします。

ワークフローの`prepare`ジョブが最初に実行され、バージョンアップ作業を代行します: バージョンが`x.y.z`形式であること(各要素に先頭ゼロがないこと含む)を検証し、新規リリースか再開かを判定し(後述の[失敗したリリースからの復旧](#失敗したリリースからの復旧)参照)、ルート`Cargo.toml`の`[workspace.package].version`を書き換え、`cargo update -w`で`Cargo.lock`を追随させ(workspace限定なので`vendor/yorishiro`にpinされた`yorishiro-core`/`yorishiro-server`のgit依存には一切触れません)、この2ファイルをコミットして`v<version>`タグと共に`master`へ直接・アトミックにpushします。
これはリポジトリ内で唯一、PRを経由せず`master`へpushする箇所です([CLAUDE.md](../../CLAUDE.md#git-workflow)参照)。

`prepare`がタグをpushした後は、それ以前と同様にそのタグに対してビルドが行われます: `yorishiro-hosted-server`の`x86_64`/`aarch64` Linux(glibc、`linux-amd64`/`linux-arm64`の`.tar.gz`として梱包)バイナリと`x86_64` Windows(`windows-amd64`の`.zip`として梱包)バイナリをビルドしてすべてGitHub Releaseに添付し、マルチアーキのDockerイメージを`ghcr.io/yotsunagi/yorishiro-hosted:v<version>`(および`:latest`)としてビルド・push -- Dockerジョブもタグ付けされたコミットをcheckoutするため、`web/`SPAもビルドし直された状態でイメージに焼き込まれます。
Linux側の2アーキテクチャはどちらもQEMUを使わずネイティブビルドします。
Windowsバイナリは Docker イメージには含まれません。

`prepare`のpushはワークフロー自身の`GITHUB_TOKEN`を使うため、これによって`master`上の`ci.yml`が再トリガーされることはありません(GitHubはデフォルトトークンによるコミットに対して`push`イベントを発火しません)。
これは意図した挙動であり、不具合ではありません。

GitHub Releaseの作成前に`smoke`ジョブが動きます。
公開したばかりのマルチアーキmanifestをpullし、本物のPostgreSQLに対して起動して、ポート8081の`/up`が応答しなければリリースを失敗させます。
`ci.yml`の`package-smoke`はPRごとにデバッグビルドを確認しますが、**実際のリリース成果物**を確認するのはこのジョブだけです(Dockerジョブが専用Nodeステージでビルドする SPA も含みます)。

### 失敗したリリースからの復旧

`prepare`はbumpコミットとタグをまとめて(atomicに)pushするため、以降のジョブで失敗すると**タグはあるがGitHub Releaseが無い**状態になります。
この場合は**同じバージョンでもう一度実行してください。**
ワークフローはタグの有無ではなく**GitHub Releaseの有無**で状態を判定します。

| 状態 | 実行時の挙動 |
|---|---|
| タグなし | 通常のリリース(bump → タグ → 公開) |
| タグあり・Releaseなし | 再開。bumpを飛ばし、既存タグから公開をやり直す |
| タグあり・Releaseあり | 明示的に失敗。そのバージョンは公開済み |

GitHub Releaseは全成果物のpushとスモークテスト通過の**後**に作成されるため、「このバージョンは出荷済み」の目印として信頼できます。
リモートタグを手で削除したり、パッチ番号を1つ飛ばしたりする必要はありません。

## public repo依存バージョンの更新

public repoが新しいタグを切ったら:

```console
$ cd vendor/yorishiro && git fetch --tags && git checkout <new-tag> && cd ../..
$ # crates/yorishiro-hosted/Cargo.tomlの`tag = "..."`(yorishiro-coreとyorishiro-serverの両方)を合わせて更新
$ cargo update -p yorishiro-core -p yorishiro-server
$ git add vendor/yorishiro crates/yorishiro-hosted/Cargo.toml Cargo.lock
```
