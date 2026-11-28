# Dependency bug fixes

This directory holds temporary fixes for specific upstream bugs.
Cargo selects the local crates through `[patch.crates-io]` in the root manifest.
Do not edit Cargo's registry cache.

Do not patch dependencies solely to replace an unmaintained library, change features, or hide an advisory.
Each patch must document the upstream version, the bug, the change, regression checks, and the condition for removing it.
Preserve upstream licences in copied source.
Distinguish maintenance warnings from vulnerabilities and confirm the cause before claiming a fix.

- [Loco queue fixes](loco-rs/README.md)
