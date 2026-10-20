# Dependency bug fixes

This directory contains temporary, reviewable fixes to upstream dependencies, not application features.
Cargo selects the local crates through `[patch.crates-io]` in the root manifest.
Do not edit Cargo's registry cache.

Only concrete bug fixes belong here.
Do not patch dependencies solely to replace an unmaintained library, change features, or hide an advisory.
Each patch must document the upstream version, the bug, the change, regression checks, and the condition for removing it.
Preserve upstream licences in copied source.
Do not describe a maintenance warning as an exploitable vulnerability, or a mitigation as a proven fix for an unrelated failure.

| Directory | Original dependency | Bug fix | Removal condition |
|---|---|---|---|
| `loco-rs/` | `loco-rs` 1.2.0 | Documented queue bug fixes, complete upstream source, reproducible diff and verifier. | See [the defect/removal checklist](loco-rs/README.md). |

The maintenance-only `paste` and YAML compatibility overrides have been removed.
Their upstream dependencies remain unchanged; maintenance warnings are not claimed resolved.

## 修正の管理

このフォルダは、具体的な上流バグの修正だけを管理します。
保守停止だけを理由にした依存の置換、機能変更、監査警告を隠すためのパッチは追加しません。
アプリ固有機能の追加ではありません。
ルートの `[patch.crates-io]` から参照し、Cargoのキャッシュは変更しません。
各修正には、対象バージョン、問題、変更内容、回帰試験、削除条件を記録します。
上流のライセンスを保持し、保守停止警告を脆弱性と呼んだり、原因未確定の障害まで解決したと記載したりしません。
保守停止への対応だけを目的にした `paste` と YAML の互換パッチは削除しました。
元の上流依存を使い、保守警告は未解消として扱います。
Locoのバグ一覧、元ソースの出所、差分の再現方法、回帰試験、削除条件は `loco-rs/README.md` に記録します。
