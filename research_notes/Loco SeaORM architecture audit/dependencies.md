# Loco / SeaORM architecture dependency audit

調査日: 2026-10-07

## 結論

- `cargo metadata --locked --format-version 1` で workspace 構成と feature を確認した。
- `cargo machete` 0.9.2 は未使用の直接依存を検出しなかった。
- `rg` で Rust / Python / Makefile / migration / `ee/` を横断し、全直接依存について利用箇所または機能上の理由を確認した。
- したがって、現時点で「削除してよい」と断定できる直接依存はない。
- ただし、Cargo.lock には Loco の内包依存とアプリの直接依存による重複バージョンがあり、依存削減より先に feature/version の整合性を確認する価値がある。

## 判断表

| 依存・領域 | 直接利用 / 役割 | Loco / SeaORM で代替 | 判断 |
|---|---|---|---|
| `loco-rs` (`worker_redis`) | HTTP/bootstrap、config、tasks、workers、Redis/SQLite/Postgres queue | 代替不可。アプリの基盤そのもの | 維持。`worker_redis` は設定で Redis/Valkey を選べるため compile-time に必要 |
| `sea-orm` / `sea-orm-migration` | entity API、ActiveModel、migration、backend 差異 | 代替不可。標準経路 | 維持。通常クエリは SeaORM を優先 |
| `sqlx` | `src/db.rs` の pool/session reset、RLS GUC、advisory lock、EE の SQLite/Postgres 接続、queue test | Loco/SeaORM の標準 bootstrap では release 時 session reset 等を公開していない | 維持。ただし raw SQL は既存の明示された例外に限定 |
| `sqlite-vec` + `libsqlite3-sys` | SQLite vector extension の登録、BLOB vector 変換 | SeaORM は extension/FFI を提供しない | 維持。`libsqlite3-sys` は `sqlite3_auto_extension` の直接 FFI 用で、未使用誤検出ではない |
| `jsonschema` | entity payload の JSON Schema validation | `schemars` は schema 生成用で、validator ではない。SeaORM/Locoにも代替なし | 維持 |
| `schemars` + `rmcp` | MCP tool 引数 schema / protocol / streamable HTTP | Loco/axum だけでは MCP protocol と schema derive を提供しない | 維持。MCP の配置は既存方針どおり `src/controllers/mcp/` |
| `serde_yaml_ng` | config/test YAML の deserialize | Loco は YAML config を使うが、アプリ側の型利用もある | 維持。deprecated `serde_yaml` は直接依存ではなく Loco の生成系経由 |
| `candle-core` / `candle-nn` / `candle-transformers` | ローカル multilingual-e5-base 推論 | HTTP embedding provider では代替できるが、in-process local provider の要件を失う | 維持。機能を外部 provider 限定にする場合のみ optional feature 化を検討 |
| `tokenizers` | XLM-R tokenizer | Candle の transitive `tokenizers 0.22.x` と直接の `0.23.x` が併存 | 直ちに削除不可。`src/services/embedding/local.rs` が直接 API を使用。Candle 側との version 同居可否を upstream 更新時に検証 |
| `reqwest` | OpenAI-compatible embedding、EE OAuth/inference/HTTP clients | Loco の reqwest は再 export 前提にできず、公開 API の直接依存が必要 | 維持。`rustls`/redirect policy 等の設定もアプリ固有 |
| `sha2` / `hex` / `rand` | API key、OAuth state、HMAC/モデル checksum | Loco の汎用機能では代替できない。`uuid` は用途が異なる | 維持 |
| `hmac` / `base64` / `jsonwebtoken` | EE Stripe、OAuth/PKCE、license/JWT | Loco 内の JWT 依存は `jsonwebtoken 10.x`、アプリは `11.x` API を直接利用 | EE feature のまま維持。`jsonwebtoken` は Loco と version が二重になるため、同じ API/feature が揃う場合のみ一本化を検証 |
| `regex` | metaschema の user regex validation | stdlib に正規表現 engine はない。SeaORM/Loco 代替なし | 維持 |
| `thiserror` / `anyhow` | `YorishiroError` と internal error wrapping | Loco にも依存するが、自 crate の public error/API で直接利用 | 維持。`YorishiroError` に集約し、入口で anyhow を漏らさない |
| `utoipa` / `utoipa-swagger-ui` | OpenAPI derive/UI（`openapi` feature） | Loco/axum は OpenAPI generator/UI を標準提供しない | 維持。community feature で有効化されることに注意 |
| dev: `axum-test`, `futures`, `proptest`, `serial_test`, `tempfile`, `tracing-subscriber` | request/test harness、property test、race isolation、temp DB/logging | 本番依存へ移す理由なし。Loco testing だけでは全機能を代替しない | dev-dependencies のまま維持 |

## 削除候補ではないが要確認の重複

1. **`jsonwebtoken` 10.x / 11.x**
   - `cargo tree -i jsonwebtoken@10.4.0` は `loco-rs` 経由。
   - `cargo tree -i jsonwebtoken@11.1.0` は本アプリ直接依存。
   - Loco の API に合わせて EE 側を 10.x に落とせるか、または Loco 更新で 11.x に揃うかを別途検証する価値がある。
   - ただし license/JWK API と rust_crypto feature の互換性未確認のため、削除・変更を即断しない。

2. **`tokenizers` 0.22.x / 0.23.x**
   - `0.22.x` は Candle 0.11.0 の transitive dependency。
   - `0.23.x` は `src/services/embedding/local.rs` が直接利用する dependency。
   - アプリ側を 0.22.x に揃えられる可能性はあるが、Candle の公開型との互換性を含むため、推測だけで変更しない。

3. **`serde_yaml` 0.9.x / `serde_yaml_ng` 0.10.x**
   - `serde_yaml_ng` はアプリと Loco が使用。
   - deprecated `serde_yaml` 0.9.x は Loco の `rrgen`/generator chain のみ。
   - アプリの直接依存を消しても Loco 側の旧版は消えないため、現状の直接依存削除候補とは扱わない。

4. **`sha2` 0.10.x / 0.11.x、`hmac` 0.12.x / 0.13.x、`base64` 0.22.x / 0.23.x**
   - SQLx/Loco/RMCP/reqwest と EE の直接利用で feature/version が分かれている。
   - lockfile の重複は観測できるが、暗号 trait の世代が異なるため、単純な Cargo.toml の一本化は危険。

## 独自インフラの Loco / SeaORM 代替性

| 独自実装 | 代替可能性 | 判断 |
|---|---|---|
| `src/db.rs` の tenant/workspace session state、`SET ROLE`/`RESET`、pool release | Loco標準DB設定だけでは不足 | 維持。RLS境界に関わるため、単なる repository 化や別ORM導入は避ける |
| `src/db.rs` の SQLite extension auto-registration | SeaORM標準では不足 | 維持。FFI境界を一箇所に閉じ込める現在の形が妥当 |
| `src/workers/` の lifecycle/admission/dispatch と Loco worker registry | Loco queue/worker は再利用できるが、DB lifecycle と edition-aware routing はアプリ固有 | 維持。ただし queue provider をもう一つ作らない |
| `src/controllers/middleware/rate_limit.rs` の token bucket | Loco/SeaORMの責務外。Tower layer 化は可能 | 現状維持。DB永続化が不要な process-local 制限であり、抽象化追加は YAGNI |
| MCP router/mount | Loco Routes では streamable HTTP service をそのまま表現しにくい | `rmcp` の `nest_service` 方針を維持。REST用の独自MCP frameworkを追加しない |
| モデル操作 | SeaORM entity API で実装済み | repository 層や汎用 DAO を追加しない |

## Raw SQL の確認

raw SQL / `Statement::from_sql_and_values` は、通常 CRUD の代替ではなく、次の範囲に集中している。

- PostgreSQL RLS、`SET ROLE`、GUC、advisory lock。
- JSONB containment、pgvector/SQLite vector、FTS/trigram、batch query。
- 既存 row の atomic update や queue/lifecycle の競合制御。
- SQLite extension 登録と backend-specific test setup。

これは SeaORM を迂回する独自 repository の乱立とは確認できなかった。新規 query はまず Entity API、必要なら raw SQL の理由を同じ箇所に残す判断でよい。

## 誤検出・限界

- `cargo machete` は macro 展開、feature 条件、文字列参照、generated code の意味を完全には判定しない。今回は `rg` と実際の import/呼び出しで主要候補を再確認した。
- `cargo metadata` の package dependency は feature を含むため、optional な `ee`/OpenAPI 依存が通常 build から消えることを表の「維持」判断に反映した。
- `rg` で依存名がコメントや Cargo.toml に出るだけの結果は利用根拠に数えていない。
- `cargo tree -d` の大量の duplicate は、Loco/SeaORM/RMCP/Candle が異なる upstream version を要求する結果であり、全てがアプリ側の重複とは限らない。
- 未実行: full `make check-all`、PostgreSQL/SQLite integration suite。依存削除の安全性検証にはこれらが必要。

## 今後の短い判断ルール

1. DBの通常読み書きは SeaORM Entity API。
2. Loco の task/worker/config/router を再実装しない。
3. `sqlx` は RLS session state、advisory lock、extension/queue-specific raw access に限定。
4. MCP は `rmcp`、JSON Schema validation は `jsonschema`、schema生成は `schemars` と責務を分ける。
5. 依存を追加する前に、Loco/SeaORM/既存の `src/db.rs`・models・workers に同じ責務がないか確認する。
6. `cargo machete` が clean でも、optional feature、version duplication、独自インフラの重複は別監査として扱う。
