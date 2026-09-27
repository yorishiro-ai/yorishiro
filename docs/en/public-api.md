# Public API Guardrail

The repository checks newly introduced broad `pub` declarations in changed handwritten Rust under `src/` and `ee/`.

Run `make public-api-check` before changing a public declaration.

Locally, the check compares the working tree with `HEAD`.
CI supplies explicit base and head refs through `PUBLIC_API_BASE_REF` and `PUBLIC_API_HEAD_REF`.
For a newly created push branch, the all-zero `github.event.before` value is treated as Git's empty tree, so the first commit is checked as entirely new.
Force-pushes are limited by the commits available to the workflow checkout and must provide reachable base and head commits.

The check compares normalized path, declaration kind, and symbol keys between the base and head versions.
Formatting and declaration order do not affect the result.

## Scope

Generated files under `src/models/_entities/` and `src/dtos/` are excluded explicitly.

Migration code, tests, binaries, dependencies, runtime behavior, and unrelated public APIs are outside the check.

This is a lightweight supported-form scanner, not a full Rust parser.

It handles attributes, generics, `pub const fn`, `pub async fn`, `pub unsafe fn`, restricted visibility, `pub use`, and public struct, enum, trait, type, const, static, and module declarations.

This is a regression guardrail, not a blanket ban on `pub` and not a second visibility-cleanup pass.

## Review Guidance

The allowlist records only newly approved declarations with a path, kind, symbol, and specific reason.

When an exception is needed, reviewers should consider these intentional boundaries and require the applicable boundary to be named in the concise reason column:

- `integration-test`: constants, helpers, fields, or types imported by the external integration-test crate.
- `openapi-transport`: utoipa, Axum, REST, and OpenAPI request, response, router, and macro surfaces.
- `mcp`: rmcp tool arguments, handlers, routers, and macro surfaces.
- `seaorm-facade`: handwritten SeaORM facades and their generated-entity boundaries.
- `auth-provider-resolver`: externally implementable authentication, provider, and resolver seams.
- `app-hooks`: `App`, `Hooks`, and application composition entry points.
- `loco-task-worker`: Loco Task and Worker registration entry types and payloads.
- `ce-ee-composition`: the `src/app.rs` base and Enterprise composition boundary.
- No historical public surface needs to be listed.

These labels are reviewer guidance only.
They are not an additional allowlist column or an automatically accepted category.

New declarations must either be narrowed to `pub(crate)`, `pub(super)`, or private visibility, or be added deliberately to `scripts/public_api_allowlist.tsv` with a concise reason naming the applicable boundary.

Do not update the allowlist just to silence a failure.
Review the caller and record why the broader visibility is required.
Macro-generated APIs and Rust forms outside the supported scanner require the same manual review and are outside this lexical guardrail.

The policy is anchored to the completed #455 inventory and merged #456 visibility tightening in PR #463 at `b492d60bea35f26f3231b982020f3f0b1a96b64e`.

Existing unchanged public declarations pass automatically because they are present in both base and head.

An unresolved base or head ref fails loudly instead of silently checking the wrong range.

The checker reports the exact path and symbol, then points to this policy and the allowlist entry to update.
