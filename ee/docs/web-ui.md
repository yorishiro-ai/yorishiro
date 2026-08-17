# The web UI

**English** | [日本語](ja/web-ui.md)

`ee/web` is the product's UI: a React SPA, and the only one.

The SPA **is** compiled into the binary, from `ee/web/dist`, via `rust-embed`.
That is what keeps the promise the README makes: one binary, and starting it gives you a UI.
`dist/` is a build output and is not committed, so a build runs `pnpm run build` in `ee/web` before `cargo build`: CI, the release workflow and the Dockerfile all do.
A checkout that skips it fails the `embeds_a_built_spa` test rather than producing a binary that serves a blank 404 at `/`.

`YORISHIRO_WEB_DIR` overrides the compiled-in copy with a directory on disk, read fresh on every request, for iterating on the UI without rebuilding the binary.

Pages that need a licence key show the API's own error when it is absent; the UI itself is not gated, since setup, login, member and workspace management are part of the free floor.

**`ee/web` is a pnpm project.** The lockfile is `pnpm-lock.yaml`, and the pnpm version is pinned in `package.json`'s `packageManager` field, which CI and the Docker image both read (via `pnpm/action-setup` and `corepack enable` respectively).
Running `npm install` there instead ignores `pnpm-lock.yaml` entirely and resolves a different dependency tree than the one that was built and tested; use `pnpm install`.
`pnpm run check` runs the same lint/format/typecheck/build sequence CI does.

Authenticated data requests go through the same bearer-key REST API documented in [`docs/api.md`](../../docs/api.md) and [this edition's api.md](api.md); the SPA carries no privileged access the API itself doesn't already enforce.
Login, signup, and the OAuth flow are the exceptions: those are exactly how a bearer key is obtained in the first place, so they're necessarily reachable without one.

## Pages

Tenant-scoped pages (no workspace in the URL):

| Route | Purpose |
|---|---|
| `/login` | Email/password auth, plus SSO via `GET /auth/oauth/authorize` when OAuth is configured (see [api.md](api.md#oauth2oidc-login)). A `422` response reveals a "Workspace ID" field and re-prompts when an account belongs to more than one workspace and the login needs disambiguating. Also parses `window.location.hash` on load to complete (`api_key=`/`email=`) or reject (`error=oauth_failed`/`error=access_denied`) an SSO redirect coming back from the identity provider |
| `/signup` | Invite-only: requires a non-empty invite token (normally pre-filled from a `?token=` query param); this is not open self-service registration |
| `/dashboard` | Tenant landing page, laid out as a Grafana-style panel grid (see below): four stat panels, an entities-per-workspace chart, and a members-by-role chart. Built from `GET /hosted/tenant/overview` plus a workspace list and per-workspace fetches to gather the chart data. The member list drives the role breakdown but is never shown as a roster |
| `/members` | Tenant membership list, plus adding a new member with an assigned role at invite time: there is no control to change or remove an existing member's role |
| `/schemas` | All schema names for the tenant ("All Schemas" tab, the default), plus a "Templates" tab (built-in template list with preview/apply and duplicate-name detection) and a "Create Custom Schema" dialog for submitting a raw JSON schema definition directly |
| `/schemas/:schemaId` | Detail for one specific schema version, addressed by its UUID (`GET /api/schemas/{schema_id}`). The schema list has a row per version, so each row links to the version it displays: addressing by name would resolve every row to the active version instead |
| `/schemas/templates/:id` | Preview of a built-in schema template before creating a schema from it |
| `/marketplace/:templateId` | One template: its schema structure graph, entity/relation type tables, every published version, and reviews. Forking happens here |
| `/marketplace` | Templates other tenants have published: a card grid with star ratings and the latest stable version, and a dialog listing versions and reviews with fork and review controls. See [api.md](api.md#template-marketplace) |
| `/workspaces` | Workspace list, creation. Schema names resolve only for the workspace you are signed in to, since schemas are workspace-scoped; every other row shows the first octet of its schema id, with the full id on hover |
| `/api-keys` | Shows the current session's identity (workspace/tenant/user ID/scope) and admin-CLI reference commands (`create-api-key`, `list-api-keys`, `revoke-api-key`); actual key issuance/revocation/listing has no REST endpoint and is done via those `yorishiro-server admin` subcommands, not this page |

Workspace-scoped pages (`/ws/:wsId/...`; every route in this group has a `:wsId` URL segment, which `RequireWorkspace` requires be present before rendering the page):

| Route | Purpose |
|---|---|
| `/ws/:wsId/dashboard` | Workspace overview, using the same panel grid: entity/relation/schema counts plus the entity quota, with relations-per-entity and remaining-quota as panel captions |
| `/ws/:wsId/schema`, `/ws/:wsId/schema/io` | Same page component (`WsSchemaPage`) for both routes: the schema summary card, version dropdown, and tab bar always render; `/schema` shows the schema definition tab and `/schema/io` shows the JSONL import/export tab, with version switching common to both (below) |
| `/ws/:wsId/entities`, `/ws/:wsId/entities/new`, `/ws/:wsId/entities/:id` | Entity list, schema-driven creation form, detail view |
| `/ws/:wsId/graph` | Two tabs: "Schema Structure" (React Flow + ELK auto-layout) and "Entity Graph" (React Flow with a hand-written radial layout, no ELK) |
| `/ws/:wsId/search` | Entity search |

## Dashboard panels (`/dashboard`, `/ws/:wsId/dashboard`)

Both dashboards are built from `Panel`, `Stat` and `UsageBar` (`components/ui/Panel.tsx`) rather than `Card`, giving the denser Grafana-style read: a small uppercase title bar, an optional right-aligned annotation in that bar (the "mean N" on the entities chart), and content that runs to the panel edge.
`Card` is still used for the navigation tiles below the panels, which are links rather than readings.

Charts stay on `recharts`, which the SPA already depends on, and take their colours from the same CSS custom properties as the rest of the UI, so both themes work without a per-chart palette.
Flow and graph views remain on React Flow: these panels do not replace them.

A `Stat` given a `limit` colours itself by how close it is: neutral below 75% of the quota, amber from 75%, destructive from 90% and anything beyond (`thresholdClass`, pinned by `thresholds.test.ts`).
`UsageBar` renders the same ratio as a bar and **renders nothing when the limit is `null`**: an unlimited quota has no bar to fill, and a full-width bar would read as "at capacity".
This is what puts `max_workspaces` and a workspace's `max_entities` on screen; both are fetched and both were previously unused.

## Schema version switching (`/ws/:wsId/schema`, `/ws/:wsId/schema/io`)

The page resolves the workspace's schema name once (via `workspace.schema_id`, falling back to any `active`-status schema), then lists every version of that name from `GET /api/schemas` (which returns all versions for the tenant, including archived ones), sorted by version descending.
A dropdown next to the schema name lets you pick any version; selecting one fetches that exact version's definition with `GET /api/schemas/{schema_id}`.

The active version is preselected.
The page becomes read-only whenever the selected version's `status` is anything other than `active` (in practice: `archived`, but also `draft` or `deprecated`, since those are the same non-`active` condition):

- An explanatory notice is shown, and the "Edit Definition" button is hidden.
- The Import/Export tab link is visually disabled and unclickable; if the user is already on `/schema/io` for a read-only version, its content is replaced with an explanatory card rather than the normal import/export controls, since importing into or exporting from a workspace is tied to the currently active schema, not an arbitrary past version.
- The schema structure graph, entity type table, relation type table, and raw JSON still render for that version, exactly as they would for the active one.

Editing (creating a new version via "Edit Definition") is only available while viewing the active version; saving always creates version `N+1` and archives the previous active version, matching the existing `POST /api/schemas` semantics.

## Schema version diff (`/ws/:wsId/schema`)

Below the entity and relation type tables, a "Version Diff" card renders a git-style diff between any two versions of the schema, using `@pierre/diffs`.
Two dropdowns (`From` and `To`) select the sides; both default to the two newest versions, so the card opens showing the most recent change.
Each side's definition is fetched with `GET /api/schemas/{schema_id}`: `GET /api/schemas` returns summaries without a `definition` body, so listing alone is not enough to diff.

Both sides are serialized with object keys sorted (`stableJson`) before diffing.
Key order in JSON is not significant and is not preserved when a definition round-trips through the server, so diffing the raw serialization would report re-ordered keys as changes.
Array order is left alone, where position is meaningful.

The card is shown for archived versions too, since a diff is a read-only view.
When the schema has only one version there is nothing to compare and the card explains that instead; when the two selected versions serialize identically it says so rather than rendering an empty diff.

## Import (`/ws/:wsId/schema/io`)

The import control accepts either a single `.jsonl` file or a `.zip` archive containing several.
Both go to the same `POST /api/import.jsonl` endpoint and get the same per-line validation and error reporting; the archive is unpacked in the browser (`fflate`) and its members are posted one at a time, so no separate server-side format exists.

Members are imported **in filename order**, and only members ending in `.jsonl` are imported.
Directory entries, `__MACOSX/` resource forks, and dotfiles are skipped: archives routinely carry those, and posting them would fail on content the user never chose to import.
Naming files so they sort in dependency order (`01-schemas.jsonl`, `02-entities.jsonl`) is how a schema is applied before the entities referencing it.

The reported totals are summed across all members.
Import is not atomic across members: if one fails, the members before it have already been applied, so the error names the failing file rather than reporting the archive as a whole.

## Which workspace a request acts on

Content endpoints scope to a workspace, and the server resolves the one recorded on the API key unless `X-Workspace-Id` says otherwise.
The SPA sends that header only when the URL names a workspace other than the key's own, which it records at sign-in.

Without it, opening a second workspace showed the **first** one's entities, schema and search results under the second one's URL, with nothing to indicate the mismatch.
Sending it unconditionally is no better: `/auth/login` issues workspace-scoped keys, and such a key is refused when the header names another workspace, so every request would fail on the workspace the user actually signed in to.

A key that cannot reach the named workspace now produces one message naming the action that resolves it (sign in again choosing that workspace), rather than the API's own wording, which leaves the reader with a header name and no next step.

## Naming entities in lists

An entity is named by the first string it carries among `title`, `name` or `label`, falling back to its id prefix (`entityLabel`).
Ids are time-ordered, so a column of id prefixes shows the same leading characters on every row created in one session: the entity list, the search results and the graph's node picker all read as identical rows without this.

Search results also show the similarity, rendered as `1 - distance`: the ordering *is* by distance, so leaving it out means the ranking cannot be judged.
Higher is a better match, which is the direction a reader expects from a search result.

## Deep links

Every route is reachable by URL, including on a full page load.
`TenantScope` forgets the remembered workspace when a tenant-level route renders, and it must do that **without navigating**: when it navigated to `/dashboard`, a full load of `/marketplace`, `/schemas/:id` or `/schemas/templates/:id` bounced to the dashboard before rendering.
Clicking through from inside the app hid it, because the effect does not re-run when only the child route changes.

`useWorkspace` therefore exposes both: `clearWorkspace` forgets without navigating, and `leaveWorkspace` forgets *and* returns to the dashboard, which is what the sidebar's "Back" means.

## Theme and colour tokens

Both themes are driven by the CSS custom properties in `web/src/index.css`: the `@theme` block defines light, and `.dark` overrides the ones that change.
`useTheme` toggles the `dark` class on `<html>` and stores the choice under `yorishiro_theme`, falling back to `prefers-color-scheme` when nothing is stored.

Every token that carries text is chosen so that text meets the WCAG AA contrast minimum (4.5:1 for body text, 3:1 for large text) **against the darkest surface it can land on**, not just against the page background.
Secondary text sits on `--color-muted` as often as on the page, so `--color-muted-foreground` is measured against `--color-muted`.

Two tokens are deliberately not shared between the themes, and both exist for contrast reasons:

| Token | Light | Dark | Why |
|---|---|---|---|
| `--color-muted-foreground` | `#66666e` | `#a1a1aa` | Secondary text. One value cannot serve both: zinc-500 reads at 4.6 on a light page but falls to 3.1 on `#27272a`. |
| `--color-link` | `#4f46e5` | `#8b93f8` | Primary used **as text**. Separate from `--color-primary` because the two pull in opposite directions: a fill dark enough to carry a white button label is too dark to read as a link on a dark surface. |

Use `text-link` for links and other primary-coloured text; `--color-primary` stays the button/accent *fill*, always paired with `--color-primary-foreground`.

Icon-only controls (the theme toggle, sign out, the graph reload buttons) carry an `aria-label`.
A `title` alone leaves the control unnamed for a screen reader.

Two colour sets are chosen per theme rather than as tokens, because they are painted onto the graph canvas as inline SVG style, which cannot resolve a CSS custom property:

- **Relation edge colours** (`relationPalette`): six hues, lightened for dark and deepened for light.
  One fixed palette fails at both ends: emerald reads at 2.4 on a light canvas, indigo at 4.0 on a dark one, and no hex in these hues clears 4.5 against both.
- **Field type colours** (`fieldTypeColor`): the same problem on card backgrounds, solved with Tailwind's `dark:` variant (`-700` for light, `-400` for dark).

Components that draw either one take `isDark` from `useIsDarkMode`, which watches the class on `<html>` so it stays correct however the theme was set.
