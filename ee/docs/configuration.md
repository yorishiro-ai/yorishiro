# Environment Variable Reference

**English** | [日本語](ja/configuration.md)

`yorishiro-hosted-server` is a single process that embeds the full community edition (`yorishiro-server`).
It reads both this repo's own variables (below) and the embedded community server's own variables (`YSR_EMBEDDING_*`, etc. -- documented in [yotsunagi/yorishiro's docs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)) -- with one exception: that document's `config.yml`/`YSR_CONFIG_PATH` support is wired into the community binary's own `main` only, which this process never runs, so a `config.yml` next to `yorishiro-hosted-server` has no effect.
Every setting must be passed as an actual environment variable.

`YORISHIRO_MAX_TENANTS` is the one exception: this binary force-sets it to `0` in its own code, so setting it in the environment has no effect.

Logging is initialized the same way as the community server's own `main` (`yorishiro_server::logging::init`), so `YSR_LOG_TARGET`/`YSR_LOG_DIR`/`YSR_SYSLOG_SOCKET` (stdout/single/daily/syslog -- see [yotsunagi/yorishiro's docs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md#logging)) apply to this binary too, rather than being fixed to JSON-on-stdout.

The database load guard is likewise started by this binary rather than by the embedded router, so `YSR_DB_LOAD_THRESHOLD` (default `0`, disabled), `YSR_DB_LOAD_SUSTAIN_SECS` (default `30`) and `YSR_DB_LOAD_POLL_SECS` (default `5`) apply here too.
It stays off unless the threshold is a positive number.
It matters that both editions are configured alike: they point at one database, so a guard only the community binary ran would be watching a pool this process is also loading.

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string (required). Used by both the embedded community server and this repo's own tenant/billing queries. Migrations are applied automatically on startup: first `vendor/yorishiro/migrations` (community edition), then this repo's own `crates/yorishiro-hosted/migrations` (enterprise-only additions: OAuth's `identity.users` columns, and `identity.stripe_processed_events` for webhook idempotency) |
| `YORISHIRO_HOSTED_BIND` | Listen address (default: `0.0.0.0:8081`). Set but empty (`YORISHIRO_HOSTED_BIND=`) falls back to the same default rather than failing to bind |
| `YORISHIRO_LICENSE_KEY` | The licence key that enables the paid features: an RS256-signed JWT, verified against a public key compiled into the binary. Unset, empty, invalid or expired all mean the same thing — the paid features are disabled and their endpoints answer `404`, while everything else runs normally. The server never refuses to start over this. See [Licence keys](#licence-keys) |
| `YORISHIRO_HOSTED_WEB_DIR` | Directory to serve this repo's admin dashboard SPA (`web/`, built with `pnpm build`) from at `/`. The Docker image presets this to `/app/web` (the bundled build output), so Docker deployments need no override. Bare-binary deployments must build `web/` separately and set this variable -- `web/` is never compiled into the binary itself (see [web-ui.md](web-ui.md)); left unset (or set to an empty string) outside Docker, `/` is served by the community edition's own embedded assets instead |

## OAuth2/OIDC login

An additional, optional way to sign in, alongside the embedded community server's own email/password `POST /auth/login`.
See [api.md](api.md#oauth2oidc-login) for the endpoints this enables.

| Variable | Description |
|---|---|
| `YORISHIRO_OAUTH_ISSUER_URL` | The identity provider's issuer URL, e.g. `https://accounts.google.com` or `https://login.microsoftonline.com/{tenant}/v2.0`. Unset (default) disables OAuth login entirely -- every `/auth/oauth/*` route returns `404 Not Found` and the Web UI's login page shows no SSO button, identical to a deployment that predates this feature |
| `YORISHIRO_OAUTH_CLIENT_ID` | OAuth client id registered with the provider. Required once `YORISHIRO_OAUTH_ISSUER_URL` is set (startup fails fast if it's missing or empty) |
| `YORISHIRO_OAUTH_CLIENT_SECRET` | OAuth client secret. Required once `YORISHIRO_OAUTH_ISSUER_URL` is set (startup fails fast if it's missing or empty). Also used to derive the HMAC key that signs the CSRF/PKCE `state` parameter passed through the provider redirect -- no separate secret is needed for that |
| `YORISHIRO_OAUTH_REDIRECT_URI` | Where the provider redirects back to after authentication. Defaults to `{YORISHIRO_HOSTED_BIND}/auth/oauth/callback` with an all-interfaces bind host (`0.0.0.0`, or `::`/`[::]` for IPv6) rewritten to `localhost` (only meaningful for local testing -- a real deployment behind a reverse proxy on a public hostname should always set this explicitly, since a browser can't reach `YORISHIRO_HOSTED_BIND` directly in that case) |

The OIDC discovery document (`{issuer_url}/.well-known/openid-configuration`) and the provider's JWKS are fetched fresh on every `/auth/oauth/authorize`/`/auth/oauth/callback` request rather than cached at startup, so a provider that rotates its signing keys or endpoints never requires a restart of `yorishiro-hosted-server`.

The discovery, JWKS, and token-exchange requests all require `https://`, and refuse to follow a redirect that would downgrade from `https://` to plain `http://` mid-request.
The one exception is a loopback host (`localhost` or a loopback IP), which is allowed over plain `http://` for local development against a provider or mock IdP with no TLS in front of it -- a real `YORISHIRO_OAUTH_ISSUER_URL` should always be `https://`.

`GET /auth/oauth/authorize` sets a CSRF cookie that binds the login flow to the browser that started it (see [api.md](api.md#get-authoauthauthorize)).
The cookie's `Secure` attribute follows `YORISHIRO_OAUTH_REDIRECT_URI`'s scheme: `https://` gets `Secure` (required for any real deployment, since a `Secure` cookie is never sent back over plain HTTP), while the `http://localhost:...` default used for local testing does not.
There is no separate variable to control this -- setting a public `https://` redirect URI is both required for the provider to be able to reach the callback at all and sufficient to get the stricter cookie attribute.

A first-time OAuth login (an identity provider `sub` never seen before, on an installation with no matching Yorishiro account) auto-provisions a new tenant, workspace, and `member`-role membership -- see [api.md](api.md#get-authoauthcallback).
This still respects `YORISHIRO_MAX_TENANTS` the same way every other tenant-creation path does, though as noted above `yorishiro-hosted-server` always force-sets that to unlimited.

`GET /auth/oauth/authorize`/`GET /auth/oauth/callback` are rate-limited by the embedded community server's own `YSR_AUTH_RATE_LIMIT_MAX`/`YSR_AUTH_RATE_LIMIT_WINDOW_SECS` (default: 10 requests per 60 seconds per client IP -- see [yotsunagi/yorishiro's docs/configuration.md](https://github.com/yotsunagi/yorishiro/blob/master/docs/configuration.md)), sharing the same quota as its own `/auth/login`/`/auth/signup`/`/setup*` routes -- see [api.md](api.md#oauth2oidc-login) for why.
`GET /auth/oauth/status` is not rate-limited.

## Stripe billing

| Variable | Description |
|---|---|
| `YORISHIRO_STRIPE_WEBHOOK_SECRET` | Stripe webhook signing secret, used to verify `Stripe-Signature` on `POST /hosted/stripe/webhook`. Unset (default) makes the endpoint return `501 Not Implemented` instead of accepting unverifiable requests -- Stripe billing is opt-in |
| `YORISHIRO_STRIPE_PRICE_PRO` | Stripe Price id that maps to the `pro` plan (5 workspaces, 50,000 entities per workspace) |
| `YORISHIRO_STRIPE_PRICE_TEAM` | Stripe Price id that maps to the `team` plan (unlimited workspaces/entities) |

Both `_PRICE_` variables are unset by default (no mapping).
A `customer.subscription.*` event with an unrecognized price id is logged and ignored rather than applied.

A tenant that has never had a Stripe subscription event applied has `plan = NULL` and no cap, the same as a self-hosted tenant.
See [api.md](api.md#post-hostedstripewebhook) for exactly which Stripe event types are handled and what they do.

## Licence keys

The paid features are enabled by a licence key in `YORISHIRO_LICENSE_KEY`.
The key is a JWT signed with RS256, carrying three claims: `sub` (who it was issued to), `plan`, and `exp` (a Unix timestamp).
It is verified against a public key compiled into the binary, so no network access and no further configuration are involved.

Four surfaces are gated: Stripe billing, OAuth2/OIDC login, the marketplace (`/api/marketplace/*`), and LLM-backed fill (`POST /api/schemas/active/{name}/infer-fill`).
Everything else — the API, MCP, the setup wizard, login, member and workspace management, and the template library — runs with no licence at all.

A gated endpoint answers `404 Not Found` when no active licence is held, the same answer a deployment gives for any capability it does not serve.
The check runs before authentication, so the answer does not depend on whether the caller holds a valid API key.

`plan` is recorded and logged but does not select features: any valid, unexpired key unlocks all four.

The marketplace and infer-fill gates are checked per request, so a key that expires while the server is running closes them without a restart.
Stripe and OAuth are configured at startup, so those two stay as they were until the process restarts.

One line at startup states which mode the process is in — the issuee, plan and expiry when a key was accepted, or that the paid features are disabled when there is none.
A key that is set but does not verify logs a warning and leaves the paid features disabled; the server still starts, because refusing to would take the free half down over a paid-feature misconfiguration.

Verification is ordinary source code and can be removed by anyone who rebuilds.
That is deliberate: the protection is `ee/LICENSE`, under which using such a build is a licence violation.

## Email

Transactional email (invite notifications, billing alerts) does not exist -- neither Stripe event handler sends any, and there is no environment variable to configure a provider (e.g. SES/Postmark).
A prior `EmailProvider` trait was removed since it had no real implementation and no caller; adding transactional email back requires both a provider implementation and wiring it into the handlers that would use it.
