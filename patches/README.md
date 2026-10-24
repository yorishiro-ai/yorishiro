# Dependency bug fixes

This directory holds only temporary fixes for specific bugs in upstream dependencies.
These patches are not application features.
Cargo selects the local crates through `[patch.crates-io]` in the root manifest.
Do not edit Cargo's registry cache.

Only concrete bug fixes belong here.
Do not patch dependencies solely to replace an unmaintained library, change features, or hide an advisory.
Each patch must document the upstream version, the bug, the change, regression checks, and the condition for removing it.
Preserve upstream licences in copied source.
Do not describe maintenance warnings as vulnerabilities or claim to have fixed failures whose causes are still unknown.

| Directory | Original dependency | Bug fix | Removal condition |
|---|---|---|---|
| `loco-rs/` | `loco-rs` 1.2.0 | Queue bug fixes, restorable upstream source, reproducible diff and verifier. | See [the defect/removal checklist](loco-rs/README.md). |

The maintenance-only `paste` and YAML compatibility overrides have been removed.
Their upstream dependencies remain unchanged; maintenance warnings are not claimed resolved.
