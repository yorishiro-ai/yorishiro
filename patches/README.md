# Dependency bug fixes

This directory contains temporary, reviewable fixes to upstream dependencies, not application features.
Cargo selects the local crates through `[patch.crates-io]` in the root manifest.
Do not edit Cargo's registry cache.

Each patch must document the upstream version, the defect or maintenance problem, the change, regression checks, and the condition for removing it.
Preserve upstream licences in copied source.
Do not describe a maintenance warning as an exploitable vulnerability, or a mitigation as a proven fix for an unrelated failure.

| Directory | Original dependency | Maintenance fix | Removal condition |
|---|---|---|---|
| `paste/` | `paste` 1.0.15, required by tokenizers, GEMM and pulp | Re-export `pastey` 0.2.3; no original macro implementation is copied. | Upstream consumers depend on a maintained macro crate themselves. |
| `serde_yaml/` | Deprecated `serde_yaml` 0.9.34, required by rrgen/Loco generator | Re-export maintained `serde_yaml_bw` 2.5.8. | rrgen stops depending on deprecated serde_yaml. |
| `serde_yaml_ng/` | `serde_yaml_ng` 0.10.0, required by Loco and fixture tests | Re-export the same pinned `serde_yaml_bw` API. | Loco adopts a maintained YAML implementation. |
| `loco-rs/` | `loco-rs` 1.2.0 | Documented queue bug fixes, complete upstream source, reproducible diff and verifier. | See [the defect/removal checklist](loco-rs/README.md). |

The three compatibility crates contain only public re-exports; the maintained implementations stay registry dependencies with checksums in `Cargo.lock`.
Their package names and versions intentionally match upstream requirements; they are not the original unmaintained implementations.
`serde_yaml_bw` uses the maintained `unsafe-libyaml-norway` fork and `granit-parser`, not the discontinued `unsafe-libyaml` package.
This addresses maintenance status, not a claim that all YAML parsers are memory-safe.
Validate both editions, configuration/fixture parsing, generator compilation and embedding/tokenizer tests when updating these overrides.
The compatibility source is licensed under MIT OR Apache-2.0, like its maintained implementations; licence texts are in this directory.

## 修正の管理

このフォルダは、上流依存のバグ修正と保守停止依存の置換を管理します。
アプリ固有機能の追加ではありません。
ルートの `[patch.crates-io]` から参照し、Cargoのキャッシュは変更しません。
各修正には、対象バージョン、問題、変更内容、回帰試験、削除条件を記録します。
上流のライセンスを保持し、保守停止警告を脆弱性と呼んだり、原因未確定の障害まで解決したと記載したりしません。
`paste` のAPIは固定した `pastey`、旧YAMLのAPIは固定した `serde_yaml_bw` に再公開で接続します。
保守停止した元の実装はコピーしません。
YAMLの置換先は保守中の `unsafe-libyaml-norway` と `granit-parser` を利用し、旧 `unsafe-libyaml` は依存グラフから除外されます。
これは保守停止への対応であり、パーサー全体のメモリー安全性を保証するものではありません。
Locoのバグ一覧、元ソースの出所、差分の再現方法、回帰試験、削除条件は `loco-rs/README.md` に記録します。
