# API reference for the paid features

**English** | [日本語](ja/api.md)

The endpoints on this page are the ones `ee/` adds.
Everything else the server offers (schemas, entities, search, auth, member and workspace management) is in [the main API reference](../../docs/api.md).

These endpoints read and write `identity.tenants`/`identity.tenant_memberships`/`identity.users` directly, in the same database the rest of the server uses.

Most of them require a licence key; see [configuration.md](configuration.md#licence-keys) for which, and for what they answer without one.

## OpenAPI

The process serves **two** OpenAPI documents, each canonical for one half of the router:

| Document | Covers |
|---|---|
| `/api-docs/openapi.json` | The core API: schemas, entities, relations, search, auth, workspaces |
| `/api-docs/hosted-openapi.json` | The endpoints documented on this page |

They are separate rather than combined because `yorishiro-server` builds and mounts the first from inside `build_app`, using a crate-private `ApiDoc` that `ee/` cannot reach or extend, and `axum::Router::merge` rejects a duplicate route path.
Both are unauthenticated, so a client can fetch either without a key.

Both are raw JSON.
A Swagger UI is served at `/docs`, which redirects to `/docs/` and renders there against the community edition's document.
Fetch the JSON directly rather than relying on the UI.

## Overriding a core route

`ee/`'s routes are matched **first**, with the core router behind them as the fallback:

```rust
hosted_router.merge(oauth_login_router).fallback_service(base_app)
```

rather than merged alongside it.
`axum::Router::merge` panics on a duplicate path, so a merged layout can only ever *add* paths the core router does not already serve.
This one can also *replace* them: a paid behaviour can take over an endpoint the core defines, without the core needing to know.

Resolution order is: `ee/`'s routes, then the core's, then the static-asset fallback.
An unmatched path reaches the SPA's `index.html`, which is what lets the SPA own client-side routing.

Two things to keep in mind when adding a route:

- **Overriding a path overrides every method on it.** A request whose path matches here but whose method does not gets this router's `405`, and never reaches the core handler for that method.
  Define every method the path needs, or leave the path alone.
- Layers do not cross the boundary.
  Each sub-router carries its own `apply_observability_layers`/`apply_body_limit_layer`.

One process serves both halves, so an override is final: there is no second process where the core's version of that route is still reachable.

## `POST /hosted/stripe/webhook`

Receives Stripe subscription events.
Verifies the `Stripe-Signature` header (HMAC-SHA256 over `{timestamp}.{body}` with `YORISHIRO_STRIPE_WEBHOOK_SECRET`, rejecting a timestamp more than 5 minutes off from the current time in either direction) before processing anything.

| Response | When |
|---|---|
| `501 Not Implemented` | `YORISHIRO_STRIPE_WEBHOOK_SECRET` is unset: the deployment hasn't configured Stripe yet |
| `400 Bad Request` | Missing/invalid `Stripe-Signature` header, or a malformed JSON body |
| `200 OK` | Event verified and applied (or was a type this deployment doesn't act on) |
| `500 Internal Server Error` | The event was valid but applying it failed (e.g. a database error) |

Event types handled:

| Event | Effect |
|---|---|
| `checkout.session.completed` | Links the Stripe customer id to the tenant named by the checkout session's `client_reference_id` (expected to be set to the tenant's UUID when the checkout session is created) |
| `customer.subscription.created` / `customer.subscription.updated` | Resolves the tenant from the linked Stripe customer id, maps the subscription's price id to a plan via `YORISHIRO_STRIPE_PRICE_PRO`/`YORISHIRO_STRIPE_PRICE_TEAM`, and applies that plan's `max_workspaces` cap |
| `customer.subscription.deleted` | Resets the tenant to the `free` plan's cap |

Any other event type returns `200 OK` without side effects (e.g. invoice events kept only for record-keeping on Stripe's side).

Every event's Stripe event id is recorded in `identity.stripe_processed_events` once applied, and a webhook delivery that repeats an event id already recorded is accepted with `200 OK` (so Stripe stops retrying it) but not re-applied.
For the three `customer.subscription.*` event types specifically, Stripe also does not guarantee delivery order, so a delivery whose own `created` timestamp is older than the last `customer.subscription.*` event already applied for the same Stripe customer is likewise accepted but not re-applied.
`checkout.session.completed` is excluded from this per-customer ordering check: it is a one-time link event with no ordering relationship to the subscription stream, and Stripe can deliver it before a `customer.subscription.created` that carries an earlier `created` timestamp for the same purchase.

## `GET /hosted/tenant/overview`

The sole read the admin dashboard's landing page needs: plan, workspace cap, usage counters, and the member list, in one round trip.

```console
$ curl localhost:8080/hosted/tenant/overview -H "Authorization: Bearer $YORISHIRO_KEY"
```

Requires the same bearer API key format `/auth/login` issues; a missing or invalid bearer token gets `401 Unauthorized`.
It also restricts access to callers whose tenant membership role is `owner` or `admin`.
A `member`-role key gets `403 Forbidden` regardless of the key's own `ApiKeyScope`, since billing/usage data is a tenant-admin concern independent of what content scopes the key happens to hold.

Response shape:

```json
{
  "tenant_id": "...",
  "plan": "pro",
  "max_workspaces": 5,
  "usage": {
    "tenant_id": "...",
    "workspace_count": 2,
    "member_count": 4,
    "entity_count": 1230
  },
  "members": [
    { "user_id": "...", "email": "...", "display_name": null, "role": "owner" }
  ]
}
```

`plan` is `null` until a Stripe subscription event has set one (a tenant that has never subscribed has no plan and no cap).
`usage` counts across every workspace the tenant owns.

## Template marketplace

Tenants share templates with each other.
`identity.templates` already carries `visibility` (`tenant` | `community`) and `fork_of`; the marketplace adds what makes a shared template safe to consume: published versions, and what other tenants thought of them.

Served by **this** edition: distributing templates between tenants is an enterprise capability whatever table stores them.

| Endpoint | Scope | Purpose |
|---|---|---|
| `GET /api/marketplace?limit=&offset=` | any valid API key | Community-visible templates across every tenant, with the latest stable version and review aggregates. Paginated (`limit` default 50, max 200; `offset` default 0), ordered by name then id |
| `GET /api/marketplace/{id}/versions` | any valid API key | Published versions, newest first. Your own drafts are included only for templates your tenant owns |
| `POST /api/marketplace/{id}/versions` | any valid API key | Publish the next version of your own template (`definition`, optional `changelog`, `status` of `draft`/`pre`/`stable`) |
| `GET /api/marketplace/{id}/reviews` | any valid API key | Reviews of a template you can see |
| `POST /api/marketplace/{id}/reviews` | any valid API key | Leave or replace your tenant's review (`rating` 1-5, optional `comment`) |
| `POST /api/marketplace/{id}/fork?version=N` | any valid API key | Copy a published version into your own library. Omitting `version` takes the latest `stable` |
| `PUT /api/marketplace/{id}/visibility` | any valid API key | List your own template in the marketplace, or take it back down |

A version number is assigned by the server, incrementing per template.
Letting a client choose it invites gaps and collisions in a sequence other tenants read as history.

**A draft is visible only to the tenant that owns it**, is never forkable, and keeps a template out of the listing entirely until something non-draft is published: a marketplace entry that 404s on install is worse than a shorter list.
A forked copy lands **private** in your own library: republishing someone else's work under your name is a decision, not a default.

Acting on a template your tenant does not own answers `404`, not `403`.
A caller that cannot act on a template should not be able to confirm it exists from the difference.

A fork is a template, not yet a schema.
Apply it with `POST /api/schemas` and its UUID as `template_id`, exactly as a built-in id is applied.

### Official listings

The built-in templates are published here too, by `yorishiro-server seed-official-templates`.
They are ordinary listings (forkable and reviewable like any other) attributed to the author `Yorishiro`.

Their publisher is a tenant row with **no members and no workspaces**: `identity.templates.tenant_id` is `NOT NULL` and the marketplace scopes ownership by it, so official listings need an owner.
Nobody can log into that tenant, because there is no membership to log in through.

The command is idempotent and meant to run on every deployment: a template already published at the same definition is left alone, and one whose definition changed in a new release publishes a *new version* rather than editing the one tenants already installed.

## Following an origin template

A schema created from a template records where it came from.
Both sides can move afterwards, and these three endpoints are how a workspace takes the template's later edits, or decides not to.

Served by **this** edition, for the same reason as the marketplace: distributing a template's changes to its copies is template distribution.
**Creating** a schema from a template is not, and belongs to the community edition: `template_id` on `POST /api/schemas`, `GET /api/templates`, and the `origin_*` columns.

| Endpoint | Scope | Purpose |
|---|---|---|
| `GET /api/schemas/upstream-changes` | read | Schemas in this workspace whose origin template has been edited since the copy was taken. Reports only |
| `GET /api/schemas/{schema_id}/merge-preview` | read | What following the template would do: every differing field as `auto_add`, `auto_update`, `keep_local` or `conflict`. Nothing is written |
| `POST /api/schemas/{schema_id}/merge` | schema | Write the merged definition as the schema's next version. Refuses if any field conflicts |

Three definitions are compared, not two: the template as it stood when copied, the template now, and this workspace's own.
With only two, an upstream addition and a local one look identical, both "present there, absent here", and following the template would silently delete the workspace's own fields.

**A conflict is a question for a person.** `merge` refuses rather than picking a side, because either choice invalidates whichever entities were written against the losing definition.
Resolve it by editing the schema, then merge.

A schema that follows no template, or one copied before merge bases were recorded, is refused rather than guessed at: substituting the current template for the missing base would read every local addition as a conflict.

These endpoints accept **both key kinds**: a workspace-scoped key names its own workspace, and a tenant-scoped one names it per request with `X-Workspace-Id`.
The versions that predate the move only ever saw the first.

**No MCP tools.** `list_upstream_changes`, `merge_preview` and `merge_apply` existed as MCP tools before the chain moved here, and the three are REST-only now.
This edition serves `/mcp` through its own wrapper around the base server, so what is absent is the three definitions rather than a way to reach the surface.

## Inferring missing values

Where `fill-defaults` writes values the schema itself states, this asks a model to propose one from what an entity already holds, for fields that are missing and have no sensible default.

Served by **this** edition: **what decides an edition is that the server makes an outbound chat completion at all**, not who pays for the call.
A bring-your-own-key design moves the cost without changing that.
The embedding providers are not the same thing and stayed: an `/embeddings` endpoint is not a chat completion, and the local ONNX provider makes no network call.

| Endpoint | Scope | Purpose |
|---|---|---|
| `PUT /api/workspace/llm-key` | schema | Store the workspace's own endpoint, model and key |
| `GET /api/workspace/llm-key` | read | Report the endpoint and model. **The key is never returned**, not even masked |
| `DELETE /api/workspace/llm-key` | schema | Remove it; `infer-fill` refuses again afterwards |
| `POST /api/schemas/active/{name}/infer-fill` | schema | Propose values. Returns a `job_id`; entities are untouched |
| `GET /api/migration-jobs/{job_id}/proposals` | read | What the model suggested |
| `POST /api/migration-jobs/{job_id}/confirm` | schema | Snapshot each entity under the same `job_id`, then apply |

**The deployment does not pay for the inference.**
A workspace configures its own credentials against any OpenAI-compatible chat-completions endpoint, Ollama and LM Studio included.

**That "any" is literal, and it is an exposure an operator has to weigh.** `base_url` is stored as given and the destination is not restricted, so a caller holding a `schema`-scoped key can point the server at any host it can reach, including services on the deployment's own network, and the request carries the entity's data and that workspace's bearer token.
On a self-hosted deployment where the operator and the tenant are the same people, that is the feature working as intended; on a multi-tenant one it lets a tenant use the server as a probe, with responses landing in proposals they can read.
**A restriction is not implemented, deliberately: choosing between an allowlist, an egress proxy, and denying private address ranges is an operator's decision, and the wrong default breaks the localhost endpoints above.**
Two narrower things are enforced: `base_url` must be `http://` or `https://`, and redirects are refused rather than followed, so a 307 cannot re-send the body and headers to a host nobody configured.

**If a restriction is added later, it has to validate the resolved IP at request time**, not the hostname when the key is stored.
A name that resolved to a public address at `PUT` can resolve to `127.0.0.1` by the time the request goes out, so a check at storage time validates a different resolution than the one used.
The reqwest seam for this is a custom DNS resolver, with any storage-time check kept only as an error message a person sees sooner.

A workspace with no key gets **422** rather than falling back to defaults: a caller who asked for inference and received `default` values would have no way to tell that nothing was inferred.

**A proposal is not a write.** Confirming snapshots through the same mechanism `POST /api/migration-jobs/{job_id}/undo` reverses, so a confirmed batch rolls back exactly as a `fill-defaults` run does.
A proposal the schema rejects is counted in `skipped` rather than failing the batch.
A job can be confirmed once: the proposals are deleted on apply, so confirming again after an undo cannot write the same guesses back over what the undo restored.

`identity.workspace_llm_keys` holds the key in plaintext and **`yorishiro_app` is granted nothing on it**: the repository reaches it through the migration-role pool, so a query arriving on a request connection fails at the permission check rather than depending on an RLS policy being correct.
Encryption at rest is the volume's or the managed database's concern: a key stored beside the data it protects, on a host the operator controls, leaks with the dump either way.

**No MCP tools.**
These six are REST-only: this edition's `/mcp` wrapper can carry tools of its own, and none of the six is defined as one.

## Tenant-scoped API keys

A plain API key binds to exactly one workspace, so a client working across several would otherwise hold one key per workspace and swap between them.
A key issued here can instead be bound to a **tenant**, naming the workspace per request with `X-Workspace-Id`.

```console
# Issue one (there is no REST endpoint for key issuance)
$ yorishiro-server create-tenant-api-key <tenant-id> write

# Every request then names its workspace
$ curl localhost:8080/api/entities -H "Authorization: Bearer $YORISHIRO_KEY" \
    -H "X-Workspace-Id: <workspace-id>"
```

This works by installing a `yorishiro_core::services::auth::Authenticator` (the seam for replacing how a key resolves), so the header is honoured on **every** authenticated path, REST and MCP alike, rather than on the routes that remembered to look.

| Key | `X-Workspace-Id` | Result |
|---|---|---|
| workspace-scoped | omitted | Acts on the key's own workspace |
| workspace-scoped | matches the key's workspace | Same as omitting it |
| workspace-scoped | names a different workspace | `422`: the key cannot act there, and silently using its own workspace would put the write somewhere the client did not name |
| tenant-scoped | omitted | `401`: there is no workspace to fall back on |
| tenant-scoped | a workspace in the key's tenant | Acts on that workspace |
| tenant-scoped | a workspace in another tenant | `401`, indistinguishable from an unknown key |
| either | not a UUID | `422` |

**A tenant-scoped key never reaches outside its own tenant.**
`identity.authenticate_api_key`'s two-argument form resolves the requested workspace only when it belongs to the key's tenant, and that check runs during authentication, before any row is read.

The original single-argument `authenticate_api_key(bytea)` is left in place: Postgres overloads on arity, so this is an addition.
That function INNER JOINs `identity.workspaces`, so a community-edition process reading the same database rejects a tenant-scoped key rather than mis-resolving it, which is the correct answer for a process with no way to be told which workspace was meant.

Scope works exactly as it does for a workspace-scoped key (`read` < `write` < `schema`, capped by the attributed user's tenant role); it just applies across every workspace in the tenant.
Prefer a workspace-scoped key when a client only ever works in one: it reaches less if it leaks.

## OAuth2/OIDC login

An additional, optional way to obtain a Yorishiro API key alongside the built-in `POST /auth/login` (email/password).
Disabled by default; see [configuration.md](configuration.md#oauth2oidc-login) for the environment variables that enable it.
When OAuth isn't configured, `GET /auth/oauth/authorize` and `GET /auth/oauth/callback` both return a `404 Not Found` JSON error body.
This is not necessarily the same response a mistyped URL gets: an *extensionless* unmatched path falls through to the Web UI's `index.html` (`200 OK`) instead of a `404` when the Web UI is being served, whereas a path with a file extension (e.g. `/foo.js`) does still get a real `404`.
`GET /auth/oauth/status` is the exception to the disabled-means-404 rule: it always answers `200` so the Web UI's login page can decide whether to show the "Sign in with SSO" button.

The per-IP rate limiter (`429` once exhausted, `YORISHIRO_AUTH_RATE_LIMIT_MAX`/`YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS`; see [the main configuration reference](../../docs/configuration.md)) is applied route-locally within each sub-router that needs it, not globally.
`axum::Router::merge` doesn't carry a `.layer()` across the merge, so a route only gets rate-limited if the sub-router it's actually defined in applies the layer before merging.
`yorishiro-server`'s `main` applies it to `GET /auth/oauth/authorize`/`GET /auth/oauth/callback` explicitly, sharing the *same* limiter instance (and so the same per-IP quota) the community server's own `POST /auth/login`/`POST /auth/signup`/`GET /setup/status`/`POST /setup` draw from.
An attacker who exhausts the quota against one can't get a fresh one by switching to the other.
`/authorize` itself never issues a key: it only redirects to the provider and sets a CSRF cookie; `GET /auth/oauth/callback` is the one route that can end up issuing a Yorishiro API key, after validating the callback (see below), from caller-supplied input (an authorization code), which is exactly why it's rate-limited the same as a login attempt.
`GET /auth/oauth/status` is deliberately *not* rate-limited: it returns no secret, and the Web UI's login page calls it on every load.

### `GET /auth/oauth/status`

```json
{ "enabled": true }
```

### `GET /auth/oauth/authorize`

Redirects (`302`) to the identity provider's own authorization endpoint (discovered from `YORISHIRO_OAUTH_ISSUER_URL`'s `.well-known/openid-configuration`), with a fresh signed `state` and PKCE (`S256`) challenge attached.
Nothing is stored server-side between this request and the callback below: the PKCE verifier round-trips inside the signed `state` value itself, so the two requests don't need to land on the same process in a load-balanced deployment.

Also sets a `ysr_oauth_csrf` cookie (`HttpOnly`; `Secure` when `YORISHIRO_OAUTH_REDIRECT_URI` is `https://`; `SameSite=Lax`; scoped to the `/auth/oauth/callback` path; expires with the `state`'s 10-minute TTL) carrying a random per-browser value whose hash is embedded in `state`.
This is what lets the callback tell "a `state` this server issued" apart from "a `state` this server issued *to the browser now presenting it*": the signature alone only proves the former.
Without the cookie, an attacker who captures a `code`/`state` pair from their own login attempt (e.g. by relaying the callback URL to a victim) could get the victim's browser to complete the attacker's login instead of the victim's own.

### `GET /auth/oauth/callback`

The identity provider's redirect target (`redirect_uri`).
Verifies `state`'s signature and freshness, then checks its embedded hash against the `ysr_oauth_csrf` cookie the browser presents (see above): a missing or mismatched cookie is rejected the same as an invalid `state`, before any request reaches the identity provider.
The cookie is cleared on every response from this endpoint, so it can't be reused across two callback attempts.
Once that passes, it exchanges the authorization `code` for tokens, verifies the ID token's signature (against the provider's JWKS) and standard claims (`iss`, `aud`, `exp`), then:

1. Looks up an existing Yorishiro user by `(provider, subject_id)`: the ID token's `sub` claim, not its `email`, since `sub` is what a provider actually guarantees stable and unique.
   This lookup does not require the ID token's `email_verified` claim to be `true`; an unverified email is still accepted for auto-provisioning (a warning is logged, but the login proceeds).
   An identity provider that never verifies email addresses is trusting the provider's account-creation flow, not this server, to prevent someone from claiming an address they don't own.
2. On first login, auto-provisions a new user (keyed to the ID token's `email` claim), a new tenant named after the email's local part, a schema from the built-in `general-notes` template, and a default workspace linked to that schema (capped at the Free plan's entity limit, since a freshly auto-provisioned tenant has no Stripe subscription yet), plus a `member`-role membership.
   A provider that omits the `email` claim can't be auto-provisioned and gets `422 Unprocessable Entity` as a direct JSON error response (not the failure redirect below).
   An `email` that's already registered under a different provider/subject gets `409 Conflict`, also as a direct JSON error response.
   The user row and its tenant membership are written in one transaction, so a request that dies between them (a dropped connection, a crashed process) rolls both back rather than leaving an unreachable user with no tenant.
   The tenant/schema/workspace created earlier in the same attempt are not part of that transaction and may still exist after such a crash.
   This is harmless, since nothing looks a tenant up by "was it ever joined", so a retry only guarantees no orphaned *user*, not a fully clean slate.
3. On a *returning* login, the existing account's tenant membership is re-checked: an account that's no longer a member of any tenant, or no longer a member of the tenant that owns its default workspace, gets `403 Forbidden` as a direct JSON error response.
4. Issues a Yorishiro API key exactly the way `POST /auth/login` does (same `create_api_key`, scope derived from the account's membership role).
5. Redirects (`302`) to `/#api_key=<key>`: the Web UI's router reads the key out of the fragment, stores it, and forwards to `#/dashboard`.
   A fragment (never sent to any server) is used specifically so the key never appears in access logs or a `Referer` header.

Only two failure modes redirect to `/#/login?error=oauth_failed` instead of returning a JSON error body, since the caller is a browser mid-redirect at that point: the identity provider's redirect carrying `error=...`, and a callback request missing `code` or `state`.
Every failure after that point instead falls through the standard JSON error envelope with its own status code, the same as any other API error: `401` for state/CSRF/token/ID-token failures, `422` for a missing email claim, `403` for a returning identity whose tenant membership no longer checks out (step 3 above), `409` for an email collision, and `500` for an unexpected failure talking to the identity provider (discovery document/JWKS fetch) or acquiring the per-identity provisioning lock (about a 2-second timeout).
A request that exceeds the rate limit (see above) never reaches any of this: it gets a bare `429` with no JSON body, the same as `POST /auth/login` does, before the CSRF/state checks even run.

Request bodies on every route in this document are capped at 2 MB.
`axum::Router::merge` doesn't carry a `.layer()` from either side to the other, so `yorishiro-server`'s `main` applies `apply_body_limit_layer` explicitly to `ee/`'s own sub-router, the same way it does for the rate limiter (see above).
Even without it, `axum`'s `Bytes`/`Json`/`String` extractors fall back to their own built-in 2 MB default whenever no explicit layer applies, which is what enforced this cap before the layer was added; the explicit layer additionally covers a hypothetical future handler that reads a raw `Request`/streaming body instead of one of those extractors.

```console
$ curl -i localhost:8080/auth/oauth/authorize
HTTP/1.1 302 Found
location: https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=...
```

## Entity table columns

The create form builds its fields from the schema.
The table that lists the results showed the same four columns for every workspace, so a schema declaring `status` and `priority` hid both until you opened a row.

| Endpoint | Scope | Purpose |
|---|---|---|
| `GET /api/workspace/entity-columns` | read | Every stored choice in the workspace |
| `PUT /api/workspace/entity-columns/{entity_type}` | write | The visible columns for one entity type, in display order |
| `DELETE /api/workspace/entity-columns/{entity_type}` | write | Forget the choice, so the schema decides again |

`write`, not `schema`: which columns a table shows is a display preference, so a key that may create entities may also decide how they are listed.

**The choice is per workspace, not per user.** Everyone looking at a workspace sees the same table.

**An absent choice and an empty one are different.** No stored row means the workspace has never chosen and the table derives its columns from the schema; a stored empty list means it chose to show none.
`DELETE` removes the row rather than storing `[]`, which is what keeps the two distinguishable.

At most 12 columns can be shown at once.
A table wider than the screen stops being a table, and a schema with sixty fields would otherwise let one click produce one.

A field name the schema no longer defines stays stored and is skipped when rendering.
Cleaning it up on write would make a schema migration responsible for display settings.

Field-level filtering uses `GET /api/entities`'s `filter` parameter, which is JSONB containment (`data @> filter`).
Containment matches exactly, so it cannot express a range or a substring, and the UI only offers an input for fields whose values are a closed set: enums and booleans.
