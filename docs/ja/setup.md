# セットアップ

[English](../setup.md) | **日本語**

## 前提条件

サーバの起動には埋め込みモデルが必要です。
既定のローカルONNXプロバイダは、モデルファイル以外の外部サービスや設定を必要としません — ただしこの手順を省略した場合、機能が縮退した状態で起動するわけではなく、`models/model.onnx`/`models/tokenizer.json`が存在しなければ(リスナーがbindする前に)プロセスの起動自体が失敗します。
リポジトリにもDockerイメージにもこれらのファイルは同梱されていません。

1. 既定のモデル(multilingual-e5-large、1024次元、100言語以上)を取得します。

   ```console
   $ mkdir -p models
   $ curl -L -o models/model.onnx \
       https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/onnx/model_quantized.onnx
   $ curl -L -o models/tokenizer.json \
       https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/tokenizer.json
   ```

OpenAI互換エンドポイントを代わりに使う場合は[embedding-providers.md](embedding-providers.md)を参照してください。

2. **PostgreSQL 18以降**を用意します。
   主キーに組み込みの`uuidv7()`を使っており、これはPostgreSQL 18で追加されたものです。
   それより古いサーバでは機能が縮退するのではなく、マイグレーション自体が失敗します。

3. `DATABASE_URL`のロールに、マイグレーションが必要とする権限を与えます。
   マイグレーションは拡張・アプリケーションロール`yorishiro_app`・全テーブルを作成するため、次の2点が必要です。

   - `vector`(pgvector)がサーバに導入済みであること。
     `pg_trgm`はPostgreSQLのcontribに同梱されるため非superuserでも作成できますが、pgvectorは同梱されておらず、その導入自体がsuperuser(あるいはパッケージ)側の作業になります。
     ロールがsuperuserでない場合は、両方の拡張を事前に作成してください — 対象データベース上か、あるいは`template1`上に作れば以降作成されるデータベースが継承します。
     マイグレーションは両方を`IF NOT EXISTS`付きで宣言するため、既に存在する拡張はそのまま通過します。
   - `SET ROLE yorishiro_app`が可能であること。
     サーバはリクエストをこのロールで処理します。
     PostgreSQL 16以降、作成したロールであっても自動では`SET ROLE`できないため、マイグレーション自身が`GRANT yorishiro_app TO CURRENT_USER`を発行します。
     superuserはメンバーシップに関わらず`SET ROLE`できるので、superuser運用ではこの経路が一度も踏まれません。

   - 対象データベース上にスキーマを作成できること。
     マイグレーションは`identity`と`content`を宣言します。
     通常はそのデータベースの所有者であれば足り、そうでなければ`GRANT CREATE ON DATABASE <名前> TO <ロール>`が必要です。
     **`CREATEDB`はこれに該当しません** — `CREATEDB`は新規データベースの作成を許可するもので、既存データベースへのスキーマ追加とは別です。
     他人が所有するデータベースを指した場合は`permission denied for database`で失敗します。

   これら以外では、非superuserの場合に`yorishiro_app`を作るための`CREATEROLE`と、データベース自体も作るなら`CREATEDB`が必要です。

以下3つの起動方法から1つを選んでください。

## Dockerで動かす

最も手早い方法です。
DockerとPostgreSQLへの接続先が必要です。

1. 上記の[前提条件](#前提条件)を済ませます。
2. DBと`models`ディレクトリを指定してコンテナを起動します。

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -v "$(pwd)/models:/app/models:ro" \
       -e DATABASE_URL=postgres://... \
       ghcr.io/yotsunagi/yorishiro:latest
   ```

3. 起動を確認します。

   ```console
   $ curl localhost:8080/up
   ```

これだけでシングルテナント構成として完全に動作します。
Web UIはバイナリに組み込み済みで、別途`web/`を取得・マウントする必要はありません。
`YORISHIRO_MAX_TENANTS`/`YORISHIRO_EMBEDDING_PROVIDER`も既定でシングルテナント・ローカルONNXの値になっており、上でマウントした`models/`と一致します。
変更方法は[configuration.md](configuration.md)を、バックグラウンド起動やソースからのイメージビルド、同じイメージでの管理CLI実行は[deployment.md](deployment.md#バックグラウンドで起動する)を参照してください。

## パッケージからインストールする

各[リリース](https://github.com/yotsunagi/yorishiro/releases)に`.deb`と`.rpm`を添付しています。
`amd64`と`arm64`、両エディション分です。
apt/yumリポジトリの追加は不要で、ファイルを直接インストールしてください。

### どちらのパッケージを入れるか

2つは互いにConflictするため、1台にはどちらか一方だけが入ります。

| パッケージ | リリースページ上のファイル | 中身 |
|---|---|---|
| `yorishiro-ee` | `yorishiro-ee_X.Y.Z_<arch>.deb`<br>`yorishiro-ee-X.Y.Z-1.<arch>.rpm` | エンタープライズ版。有償機能とWeb UIを含みますが、どちらも`YORISHIRO_LICENSE_KEY`を設定するまで無効のままなので、キーが無ければコミュニティ版とまったく同じ挙動になります。下の行に当てはまらない限り**こちらを入れてください**。 |
| `yorishiro-ce` | `yorishiro-ce_X.Y.Z_<arch>.deb`<br>`yorishiro-ce-X.Y.Z-1.<arch>.rpm` | コミュニティ版。プロプライエタリなコードを一切置けない配備向けです。**headless**——Web UIは有償側の資産なので`/`では何も配信しません。REST API・MCPサーバ・管理CLIは同一です。 |

エディションの区別はパッケージ名だけです。
どちらも`/usr/bin/yorishiro-server`を置き、`yorishiro.service`を同梱し、`/etc/yorishiro/`を読みます。
一方向けに書いた手順書・監視設定・`systemctl`コマンドは、もう一方でもそのまま通用します。
rpmでは`rpm -qi`がライセンスを返し、エンタープライズ版は`BUSL-1.1 AND LicenseRef-Yorishiro-EE`、コミュニティ版は`BUSL-1.1`です。
debにはライセンス欄が無いため、パッケージ名と説明文がその役割を担います。

### インストール

```console
$ sudo dpkg -i yorishiro-ee_X.Y.Z_amd64.deb  # または: sudo rpm -i yorishiro-ee-X.Y.Z-1.x86_64.rpm
$ sudoedit /etc/yorishiro/config.yml         # 最低限 database_url
$ sudo systemctl enable --now yorishiro
```

サービスはパッケージが作成する`yorishiro`システムユーザーで動作し、状態は`/var/lib/yorishiro`に置きます。

`DATABASE_URL`を設定する前に有効化した場合、unitは`status=78/CONFIG`で`failed`になり停止します。
`journalctl -u yorishiro`にどのファイルを編集すべきかが出ます。
待っても設定は現れないため、再試行はしません。
一方、データベースがまだ起動していないだけの場合は逆で、5秒ごとに再試行します。
同じホストのPostgreSQLと同時に起動する構成でも自力で復帰します。

### エディションを切り替える

もう一方のパッケージを入れるだけです。
既に入っている方を置き換えます。

```console
$ sudo dpkg -i yorishiro-ce_X.Y.Z_amd64.deb  # または: sudo rpm -U yorishiro-ce-X.Y.Z-1.x86_64.rpm
```

他に必要な作業はありません。
`/etc/yorishiro/`と`/var/lib/yorishiro`はエディションではなくデプロイに属し、unit名も有効化状態もそのまま、`/usr/bin/yorishiro-server`が差し替わります。
反映するにはサービスを再起動してください。

コミュニティ版へ移るとWeb UIと有償機能は失われますが、データベースには手を触れないため、戻せば再び使えます。

### ダウンロードの検証

パッケージにGPG署名は付けていません。
8つすべてを含む`checksums.txt`を各リリースに添付しているため、ダウンロードしたものが公開物と一致するかは確認できます。

```console
$ curl -LO https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/checksums.txt
$ sha256sum --check --ignore-missing checksums.txt
```

これはファイルが公開物と同一であることの確認です。
**どこで作られたか**——どのワークフローが、どのコミットからビルドしたか——は、各パッケージに付随するbuild provenanceで確認できます。

```console
$ gh attestation verify yorishiro-ee_X.Y.Z_amd64.deb --repo yotsunagi/yorishiro
```

鍵の取り込みは不要です。
attestationは公開されたtransparency logに記録され、`gh`がこのリポジトリと突き合わせます。

### 動作環境

パッケージは**glibc 2.38以降**を要求します(Ubuntu 24.04・Debian 13・Fedora 39以降)。
この下限はYorishiro自身ではなく、埋め込みプロバイダがリンクするONNX Runtimeに由来します。
パッケージの依存として宣言しているため、それより古いシステムではapt/dnfが導入を拒否し、起動できないバイナリが入ってしまうことはありません。

この2点はプルリクエストごとに、パッケージを読むのではなく実際にインストールして検証しています。
`packaging/test-install.sh`がUbuntu 24.04とFedora 39——glibcがちょうど2.38、サポート下限そのものの環境です——で導入と起動を確認し、下限未満のUbuntu 22.04とRocky 9が理由を明示して拒否することも要求します。
systemdでしか確認できない部分は`packaging/test-systemd.sh`が受け持ちます。
未設定の起動が再試行せず止まること、設定済みなら`/up`を配信すること、再起動後に自力で復帰することの3点です。

どちらもビルド済みパッケージの置かれたディレクトリを受け取り、Dockerを必要とします(systemd側は特権コンテナも必要です)。
CIも同じ2本を実行するため、CIの失敗は手元で再現できます。
パッケージのビルドはリポジトリのルートで`nfpm package --config packaging/nfpm-yorishiro.yaml --packager deb --target dist/`を実行し(`nfpm`は`src:`をカレントディレクトリ基準で解決します)、`--packager rpm`と`nfpm-yorishiro-ce.yaml`についても同様に繰り返します。
ホストのglibcが下限以下でない限り、バイナリはコンテナ内でビルドしてください。
新しいglibcでは、パッケージが宣言していないシンボルを要求するバイナリができてしまいます。

### パッケージの外でバイナリを動かす

ベアメタル/VMへの導入はパッケージが正規の手段です。
中身は同じバイナリで、加えてサービスユーザー・状態ディレクトリ・systemd unitが揃い、glibc下限を宣言しているため動かせないシステムではパッケージマネージャが導入を拒否します。
単体のtarballは配布していません。
リリースに添付されるのは8つのパッケージ(2エディション×2アーキ×2形式)とチェックサムだけです。

`/usr/bin`以外の場所でバイナリを動かしたい場合は、パッケージから取り出し(`dpkg-deb -x`、`rpm2cpio | cpio -id`)、手順1の`models/`をその隣に置いてください。
取り出すのはどちらのエディションでも`usr/bin/yorishiro-server`です。
設定は起動時の作業ディレクトリに置く`config.yml`、または`YORISHIRO_CONFIG_PATH`で別の場所を指定して行います([configuration.md](configuration.md#configyml)と[`config.example.yml`](../../config.example.yml)参照)。
パッケージ同梱のunitを使わずに再起動をまたいで動かし続ける方法は[deployment.md](deployment.md#バックグラウンドで起動する)を参照してください。

## ソースから動かす(Docker Compose)

ローカル開発向けです。
Docker、Docker Compose、makeが必要です。

1. リポジトリをcloneしてから、その中で上記の[前提条件](#前提条件)を済ませます。

   ```console
   $ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro
   # (上記と同様にmodels/model.onnx、models/tokenizer.jsonを配置)
   ```

2. イメージをビルドし(上記のリリースイメージと同じマルチステージ`Dockerfile`を使用)、PostgreSQLと`app`を起動します。
   `docker-compose.yml`は既に`app`を上記のローカルONNXプロバイダに向けています。

   ```console
   $ make init
   ```

上記3つの方法いずれで使う`-e`/環境変数も`config.yml`ファイルで代用できます(Dockerなら`/app/config.yml`にマウント)。
長い`-e`の羅列より便利です。
詳細は[configuration.md](configuration.md#configyml)と[`config.example.yml`](../../config.example.yml)を参照してください。

## エンドポイント

起動時にマイグレーションが自動適用されます(上記3つの方法いずれでも共通)。

| パス | 内容 |
|---|---|
| `http://localhost:8080/up` | Liveness probe。プロセスが起動していれば依存関係を見ず常に200 |
| `http://localhost:8080/health` | Readiness check。DB接続も確認し、障害時は503 |
| `http://localhost:8080/` | Web UI。バイナリに組み込み済み。実ディレクトリから配信させる場合は[configuration.md](configuration.md)の`YORISHIRO_WEB_DIR`を参照。何をカバーするかは下記[Web UI](#web-ui)を参照 |
| `http://localhost:8080/docs` | Swagger UI(REST APIドキュメント) |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI仕様 |
| `http://localhost:8080/mcp` | MCPエンドポイント(Streamable HTTP) |
| `http://localhost:8080/whoami` | 認証確認。ワークスペース・テナント・scopeを返す |

## 初回セットアップ

`YORISHIRO_MAX_TENANTS`が実際の上限として解決されるデプロイ(既定は未設定で`1`)は、`http://localhost:8080/`でセットアップウィザードを配信します。
管理CLIは不要です。

ウィザードはHTTP経由で配信します。
配信する主体を起動するのに、まずデータベースへ到達できる必要があるからです。
データベースが未設定のまま起動したサーバーは、どのファイルに設定すべきかを示して終了します。
接続文字列を設定して起動すれば、そこから先はウィザードがカバーします。

まだテナントが存在しない初回アクセス時は、メールアドレスとパスワードに加えて、任意入力の表示名と、`default`ワークスペースの元になる任意選択のスキーマテンプレートを入力するフォームが表示されます。
送信するとテナント・`default`ワークスペース・ownerアカウントが一括作成され、発行されたAPIキーが画面に表示されます(他のキー同様、表示は一度だけ)。
以降は同じページがログインフォームになります。

同じフローはブラウザなしでも利用できます。

```console
$ curl localhost:8080/setup/status
{"setup_required":true}
$ curl -X POST localhost:8080/setup -H "Content-Type: application/json" \
    -d '{"email":"owner@example.com","password":"a strong password"}'
{"user_id":"...","email":"owner@example.com","tenant_id":"...","workspace_id":"...",
 "api_key":"ysr_..."}
```

`POST /setup`は既にテナントが存在する場合は`409`を、`YORISHIRO_MAX_TENANTS`が無制限に解決されるデプロイ(明示的に`0`を設定した場合)では`404`を返します。
後者はサインアップ・招待でテナントを増やします。
詳しくは[サインアップ・ログイン・メンバー・ワークスペース管理](#サインアップログインメンバーワークスペース管理)を参照してください。
下記の管理CLIは、ウィザードがカバーしない操作(追加のワークスペース/テナント、招待、キーのローテーション)に引き続き使えます。

## テナント・ワークスペース・ユーザー

Yorishiroの制御プレーンは2階層構造です。

- **テナント**は組織/アカウントです。
  `max_workspaces`という課金上限を設定できます(デフォルトは`NULL`で無制限。セルフホスト運用に適します)。
  任意数の人間の**ユーザー**をロール(`owner`/`admin`/`member`/`viewer`)付きのメンバーシップとして紐付けられ、1人のユーザーが複数のテナントに所属することもできます。
  **スキーマ**はワークスペース単位で所有されます。
  各ワークスペースが自分のスキーマを持つため、片方を編集しても他方には及びません。
- **ワークスペース**はちょうど1つのテナントに属する、実際の操作対象コンテナです。
  エンティティ・リレーション・APIキーはワークスペースに紐付きます。
  ワークスペースは1つのスキーマを参照し(`schema_id`)、`max_entities`という上限も設定できます(デフォルト`NULL`/無制限)。

テナントとワークスペースを分けることで、1つの組織が複数の独立したプロジェクト(環境別・チーム別のワークスペースなど)を新規テナントを都度作らずに運用でき、スキーマがワークスペース単位であるため、あるワークスペースが構造を変えても他のワークスペースに強制されません(同名のスキーマを別バージョンで持つこともできます)。
複数人でメンバーシップを介して同一テナントの管理権限を共有できます。
テナントの**作成**は管理CLI(`DATABASE_URL`を持つ者)からのみ可能です。
ワークスペースの作成もそこから行えますが、テナントにAPIキーを持つowner/adminが1人でもいれば、それ以降はRESTからも追加のワークスペースを作成できます。
日々の**メンバーシップ**管理(招待・追加・一覧)も同様にテナントのowner/adminであればRESTから行えます。
詳しくは[サインアップ・ログイン・メンバー・ワークスペース管理](#サインアップログインメンバーワークスペース管理)を参照してください。

デフォルト(`YORISHIRO_MAX_TENANTS`未設定)では、1つのデプロイはテナント1つに制限されます。
`admin create-tenant`とサインアップフローは2つ目のテナントを作れません。
無制限にするには`YORISHIRO_MAX_TENANTS=0`を、特定数までにするにはその数を設定してください([configuration.md](configuration.md)参照)。

## テナント・ワークスペース・APIキーの発行

セットアップウィザードを使ったデプロイは、この節を飛ばせます。
既定の`YORISHIRO_MAX_TENANTS=1`の下では最初かつ唯一のテナントになるためです。
追加のテナント/ワークスペースの発行や、`YORISHIRO_MAX_TENANTS`が無制限に解決されるデプロイでの発行(この場合ウィザードは無効です)には、引き続きこの節の手順が唯一の方法です。

APIキーはDBにSHA-256ハッシュ、ユーザーパスワードはargon2ハッシュでのみ保存されます。
どちらも手作業のSQLでは発行できないため、管理CLIで行います。

```console
$ make admin ARGS="create-tenant my-team"
tenant created
  id:            019f565d-f1e3-7afb-b876-b7003e43c230
  name:          my-team
  max_workspaces: unlimited

next steps:
  1. create a schema (via REST API or --template)
  2. admin create-workspace 019f565d-f1e3-7afb-b876-b7003e43c230 <name> --schema-id <id>

$ make admin ARGS="create-api-key <workspace-id> write"
api key created (the plaintext key is shown ONLY once — store it now)
  key:          ysr_928e48292888_ef72...
  ...

$ make admin ARGS="list-tenants"
```

`create-tenant <name> [--max-workspaces <n>] [--template <id>]`は既定ではテナントのみを作成します。
`--max-workspaces`でそのテナントが作成できるワークスペース数の上限を設定できます(省略時は無制限)。
作業を開始するには、まずスキーマを作成します — 組み込みテンプレートからでも独自の定義からでも、REST API・MCP・Web UI(スキーマ一覧の「Create Custom Schema」)のいずれでも可能です — 次に`--schema-id`を指定してワークスペースを作成します。
各ワークスペースは1つのスキーマと1:1で紐付きます。
平文キーは発行時に一度だけ表示されます。
管理コマンドは`DATABASE_URL`の接続ロールで直接DBへアクセスします。
これはマイグレーションと同じ管理ロールで、`identity.tenants`/`identity.users`/`identity.tenant_memberships`に書き込める唯一のロールです(アプリ自身の`yorishiro_app`ロールにはこの権限がありません)。

`create-tenant`は`--template <id>`も受け付けます。
組み込みテンプレートからスキーマを作成し、それに紐づくデフォルトワークスペースを自動作成します。
このフラグなしではテナントのみが作成されます(ワークスペースなし、ログイン不可)。
例: `admin create-tenant acme --template general-notes`

その他の管理コマンド:

| コマンド | 内容 |
|---|---|
| `admin create-tenant <name> [--max-workspaces <n>] [--template <id>]` | テナントを作成。ワークスペース数の上限設定や、テンプレートからのスキーマ/ワークスペース自動作成も可能 |
| `admin list-tenants` | 全テナントの一覧 |
| `admin create-workspace <tenant-id> <name> [--schema-id <id>] [--max-entities <n>]` | テナント配下に追加のワークスペースを作成（スキーマとの紐づけも可能） |
| `admin list-workspaces <tenant-id>` | テナントのワークスペース一覧 |
| `admin create-user <email> <password> [--display-name <name>]` | 人間のユーザーアカウントを作成 |
| `admin add-member <tenant-id> <user-id> <role>` | ユーザーのテナントへのメンバーシップを追加、または既存のroleを変更(`owner`/`admin`/`member`/`viewer`) |
| `admin list-members <tenant-id>` | テナントのメンバーとそのroleの一覧 |
| `admin create-invite <tenant-id> <email> <role> [--ttl-hours <n>]` | 指定したメールアドレスがテナントに参加するための招待トークンを発行(デフォルトTTL: 7日)。詳細は後述 |
| `admin create-api-key <workspace-id> <scope> [--user <user-id>]` | APIキーを発行。`--user`でメンバーに紐付け可能 |
| `admin list-api-keys <workspace-id>` | キーの一覧(ID・scope・prefix・紐付けユーザー・最終使用日時) |
| `admin revoke-api-key <key-id>` | キーの即時失効(漏洩時など) |
| `admin resync-embeddings <workspace-id>` | embedding未生成のentityを再同期(同期失敗からの回復) |

## 認証とscope

すべてのAPIは`Authorization: Bearer <APIキー>`で認証します。
キーは`ysr_`で始まる文字列で、発行時に一度だけ表示されます(DBにはSHA-256ハッシュのみ保存)。

scopeは`read` < `write` < `schema` < `migration`の4段階で、上位は下位を兼ねます。
`migration`は一括移行とその取り消しに必要です——これらは既に保存された行を書き換える操作であり、まだ何も書かれていないバージョンを足すだけのスキーマ登録とは別種の権限として扱います。

### キーをユーザーに紐付ける

人間の操作も自動化も、最終的にはすべてAPIキーで認証されます。
サーバ側にcookie/セッション状態はありません。
ただしキーは人間のユーザーに**紐付け**でき、マルチユーザーのアクセス制御はセッションではなくその紐付けとユーザーのテナントroleを結びつける形で実現しています。

`create-api-key`に`--user <user-id>`を渡すとそのメンバーにキーが紐付き、要求できるscopeは`MembershipRole::max_scope()`で上限が決まります。
`owner`/`admin`は`migration`まで、`member`は`write`まで、`viewer`は`read`まで発行可能です。
この上限を超えるscopeの要求や、ワークスペースの所属テナントのメンバーでないユーザーへの紐付けは、発行時点で拒否されます。
このチェックはキー発行時に一度だけ行われ、キー自体のscopeと同様にリクエストのたびには再評価されません。
メンバーシップを剥奪しても、発行済みのキーのscopeは遡って狭まりません。
その場合はキー自体を失効させてください。

サービス・自動化用の紐付け不要なキーには`--user`を省略してください。
roleによる上限はかかりません。
`GET /whoami`はワークスペース・テナント・scopeに加えて、紐付けられた`user_id`(未紐付けなら`null`)も返します。

`POST /auth/login`(後述)は`admin create-api-key --user`のセルフサービス版です。
`DATABASE_URL`へのアクセスではなくパスワードで認証し、呼び出し元自身のroleに上限を設定済みのキーを発行します。

## サインアップ・ログイン・メンバー・ワークスペース管理

アカウント作成は招待制のみで、公開・無認証のセルフサインアップはありません。
テナントのowner/adminが招待を発行し、招待された人がそれを一度だけ使ってアカウントを作成します。
それ以降はメールアドレス/パスワードで認証してAPIキーを取得します。

1. 招待

   テナントのowner/adminがメールアドレスとroleに対して招待トークンを作成します。

   ```console
   $ make admin ARGS="create-invite 019f565d-f1e3-7afb-b876-b7003e43c230 newperson@example.com member"
   invite created (the plaintext token is shown ONLY once — send it now)
     token:      c8b9ea1f...
     ...
     expires at: 2026-07-20 16:57 UTC
   ```

   - 平文の`token`は帯域外(メール・チャット等)で招待された人に送ってください。
     APIキー同様、表示は一度だけで、DBにはハッシュのみ保存されます。
   - `--ttl-hours`(デフォルト7日)経過時か使用済みになった時点のいずれか早い方で失効します。

2. サインアップ

   招待された人がトークンを使ってアカウントを作成します。

   ```console
   $ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
       -d '{"invite_token":"c8b9ea1f...","password":"a strong password","display_name":"New Person"}'
   {"user_id":"...","email":"newperson@example.com","tenant_id":"...","role":"member",
    "workspaces":[{"id":"...","name":"default"}]}
   ```

   これにより`identity.users`の行が作成され、招待で指定されたメンバーシップも追加されます。
   同じ(既に消費済みの)トークンでの2回目のサインアップは拒否されます(422)。

3. ログイン

   以降、ユーザーはパスワードと引き換えに新しいAPIキーを取得します。
   キーは1つのワークスペースにスコープされ、自身のroleの`max_scope()`で上限が設定されます(前述参照)。

   - `workspace_id`は省略可能です。
     アカウントがちょうど1つのワークスペースにしかアクセスできない場合(既定のデプロイは常にこれに該当します)は自動解決されます。
   - 複数のワークスペースに所属している場合のみ明示的な指定が必要で、その場合は422が返ります。

   ```console
   $ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
       -d '{"email":"newperson@example.com","password":"a strong password"}'
   {"api_key":"ysr_...","api_key_id":"...","workspace_id":"...","scope":"write","user_id":"..."}
   ```

   ログインのたびに既存キーの再利用ではなく*新しい*キーが発行されます。
   不要になった古いキーは`admin revoke-api-key`で失効させてください。

4. メンバー管理

   認証後は、テナントのowner/adminは`DATABASE_URL`/管理CLIを使わずRESTでメンバーの一覧・追加ができます。

   ```console
   $ curl localhost:8080/api/members -H "Authorization: Bearer $YORISHIRO_KEY"
   $ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YORISHIRO_KEY" \
       -H "Content-Type: application/json" \
       -d '{"email":"existing-user@example.com","role":"admin"}'
   ```

   - `POST /api/members`は既存のアカウント(サインアップ済みのもの)を呼び出し元のテナントに追加するだけで、新規アカウントは作成しません。
     まだアカウントを持たない人を招き入れるには、代わりに招待(手順1)を発行してください。
   - 両エンドポイントとも、呼び出し元自身のキーがOwner/Adminメンバーに紐付いている必要があります。
     Memberロールのキーはそのキー自身のscopeに関わらず403で拒否されます。
     メンバー管理はscopeではなくテナントroleの問題だからです。

5. ワークスペース管理

   同様に、認証済みのメンバーであれば誰でもテナントのワークスペース一覧(エンティティ・リレーション・スキーマの件数を含む)を取得できます。
   作成・削除はメンバー管理と同じくowner/adminに限定されます。

   ```console
   $ curl localhost:8080/api/workspaces -H "Authorization: Bearer $YORISHIRO_KEY"
   $ curl -X POST localhost:8080/api/workspaces -H "Authorization: Bearer $YORISHIRO_KEY" \
       -H "Content-Type: application/json" -d '{"name":"staging"}'
   $ curl localhost:8080/api/workspaces/$WORKSPACE_ID -H "Authorization: Bearer $YORISHIRO_KEY"
   $ curl -X DELETE localhost:8080/api/workspaces/$WORKSPACE_ID -H "Authorization: Bearer $YORISHIRO_KEY"
   ```

   - ワークスペースを削除すると配下の全て(エンティティ・リレーション・スキーマ・APIキー)も削除されます。
     テナントに残る唯一のワークスペースは削除できません(409)。
     `DATABASE_URL`へのアクセスなしには代わりのワークスペースを発行する手段がないためです。
   - Web UI(`/`)でもログイン後に同じ作成・一覧・削除・詳細表示の操作ができます。
     詳しくは下記[Web UI](#web-ui)を参照してください。

## Web UI

初回セットアップ・ログイン・メンバー/ワークスペース管理(上記)に加えて、Web UIでは以下も操作できます。

- **スキーマ**: テナントに登録されたスキーマと、スキーマごとのentity type一覧を閲覧できます。
- **エンティティ**: ワークスペースのエンティティを閲覧・絞り込み・ページングできます。
  詳細画面ではリレーションが表示され、個々のデータフィールド(JSON)を編集できます。
  エンティティ・リレーションの**作成**や削除、組み込みテンプレートの適用を超えるスキーマ作成はできません(上記[テナント・ワークスペース・APIキーの発行](#テナントワークスペースapiキーの発行)参照)。
  それらはREST APIまたはMCP経由で行います。
- **テンプレートライブラリ**: テナントのDB保存テンプレートの一覧・作成・削除ができます(api.mdの[テンプレートライブラリ](api.md#テンプレートライブラリ)参照。フォークはREST/MCP限定でUIには未搭載)。

完全なデータ管理UIではありません — Web UIがカバーしない部分はREST API(`/docs`のSwagger UI)とMCPツールで補ってください。
