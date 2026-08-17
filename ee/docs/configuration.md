# Environment Variable Reference

**English** | [日本語](ja/configuration.md)

The variables below are the ones the paid features under `ee/` read.
Everything the rest of the product reads is in [the main configuration reference](../../docs/configuration.md), and a single process reads both.

`config.yml` and `YORISHIRO_CONFIG_PATH` apply here as well, since there is one binary and it loads the file itself.

## The `YSR_` prefix is deprecated

Every variable is named `YORISHIRO_*`.
The old `YSR_*` names, and the `YORISHIRO_HOSTED_*` names that distinguished a binary with no counterpart, are still accepted: the server copies each onto its replacement at startup and prints a warning naming both.
`YSR_WEB_DIR` and `YORISHIRO_HOSTED_WEB_DIR` both become `YORISHIRO_WEB_DIR`, which is the one setting they always were.
Setting the new name alongside an old one uses the new value.

| Variable | Description |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string (required), shared by the whole server. The one `migrations/` directory, which holds the paid tables too, is applied automatically on startup |
| `YORISHIRO_BIND` | Listen address (default: `0.0.0.0:8080`). Set but empty (`YORISHIRO_BIND=`) falls back to the same default rather than failing to bind |
| `YORISHIRO_LICENSE_KEY` | The licence key that enables the paid features: an RS256-signed JWT, verified against a public key compiled into the binary. Unset, empty, invalid or expired all mean the same thing at startup: the paid features are disabled, while everything else runs normally. The server never refuses to start over this. What a disabled feature answers differs per feature; see [Licence keys](#licence-keys) |
| `YORISHIRO_WEB_DIR` | Serves the SPA from a directory on disk instead of the copy compiled into the binary, read fresh on every request. Unset (the default) serves the compiled-in copy, which is what a normal deployment wants; see [web-ui.md](web-ui.md) |

## OAuth2/OIDC login

An additional, optional way to sign in, alongside the built-in email/password `POST /auth/login`.
See [api.md](api.md#oauth2oidc-login) for the endpoints this enables.

| Variable | Description |
|---|---|
| `YORISHIRO_OAUTH_ISSUER_URL` | The identity provider's issuer URL, e.g. `https://accounts.google.com` or `https://login.microsoftonline.com/{tenant}/v2.0`. Unset (default) disables OAuth login entirely: every `/auth/oauth/*` route returns `404 Not Found` and the Web UI's login page shows no SSO button, identical to a deployment that predates this feature |
| `YORISHIRO_OAUTH_CLIENT_ID` | OAuth client id registered with the provider. Required once `YORISHIRO_OAUTH_ISSUER_URL` is set (startup fails fast if it's missing or empty) |
| `YORISHIRO_OAUTH_CLIENT_SECRET` | OAuth client secret. Required once `YORISHIRO_OAUTH_ISSUER_URL` is set (startup fails fast if it's missing or empty). Also used to derive the HMAC key that signs the CSRF/PKCE `state` parameter passed through the provider redirect; no separate secret is needed for that |
| `YORISHIRO_OAUTH_REDIRECT_URI` | Where the provider redirects back to after authentication. Defaults to `{YORISHIRO_BIND}/auth/oauth/callback` with an all-interfaces bind host (`0.0.0.0`, or `::`/`[::]` for IPv6) rewritten to `localhost` (only meaningful for local testing; a real deployment behind a reverse proxy on a public hostname should always set this explicitly, since a browser can't reach `YORISHIRO_BIND` directly in that case) |

The OIDC discovery document (`{issuer_url}/.well-known/openid-configuration`) and the provider's JWKS are fetched fresh on every `/auth/oauth/authorize`/`/auth/oauth/callback` request rather than cached at startup, so a provider that rotates its signing keys or endpoints never requires a restart of `yorishiro-server`.

The discovery, JWKS, and token-exchange requests all require `https://`, and refuse to follow a redirect that would downgrade from `https://` to plain `http://` mid-request.
The one exception is a loopback host (`localhost` or a loopback IP), which is allowed over plain `http://` for local development against a provider or mock IdP with no TLS in front of it.
A real `YORISHIRO_OAUTH_ISSUER_URL` should always be `https://`.

`GET /auth/oauth/authorize` sets a CSRF cookie that binds the login flow to the browser that started it (see [api.md](api.md#get-authoauthauthorize)).
The cookie's `Secure` attribute follows `YORISHIRO_OAUTH_REDIRECT_URI`'s scheme: `https://` gets `Secure` (required for any real deployment, since a `Secure` cookie is never sent back over plain HTTP), while the `http://localhost:...` default used for local testing does not.
There is no separate variable to control this: setting a public `https://` redirect URI is both required for the provider to be able to reach the callback at all and sufficient to get the stricter cookie attribute.

A first-time OAuth login (an identity provider `sub` never seen before, on an installation with no matching Yorishiro account) auto-provisions a new tenant, workspace, and `member`-role membership; see [api.md](api.md#get-authoauthcallback).
This still respects `YORISHIRO_MAX_TENANTS` the same way every other tenant-creation path does, so a self-hosted deployment on the default cap of `1` refuses a second tenant rather than provisioning one.

`GET /auth/oauth/authorize`/`GET /auth/oauth/callback` are rate-limited by `YORISHIRO_AUTH_RATE_LIMIT_MAX`/`YORISHIRO_AUTH_RATE_LIMIT_WINDOW_SECS` (default: 10 requests per 60 seconds per client IP; see [the main configuration reference](../../docs/configuration.md)), sharing one quota with `/auth/login`/`/auth/signup`/`/setup*`; see [api.md](api.md#oauth2oidc-login) for why.
`GET /auth/oauth/status` is not rate-limited.

## Stripe billing

| Variable | Description |
|---|---|
| `YORISHIRO_STRIPE_WEBHOOK_SECRET` | Stripe webhook signing secret, used to verify `Stripe-Signature` on `POST /hosted/stripe/webhook`. Unset (default) makes the endpoint return `501 Not Implemented` instead of accepting unverifiable requests: Stripe billing is opt-in |
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
Everything else (the API, MCP, the setup wizard, login, member and workspace management, and the template library) runs with no licence at all.

The marketplace and infer-fill are gated per request: without an active licence they answer `404 Not Found`, the same answer a deployment gives for any capability it does not serve.
That check runs before authentication, so the answer does not depend on whether the caller holds a valid API key.
Because it runs per request, a key that expires while the server is running closes those two without a restart.

Stripe and OAuth are gated differently: an unlicensed process simply does not configure them, so they behave exactly as they do on a deployment that never set their variables: `/hosted/stripe/webhook` answers `501 Not Implemented` and every `/auth/oauth/*` route answers `404`.
That is decided once at startup, so a key that expires mid-run leaves those two configured until the process restarts.

`plan` is recorded and logged but does not select features: any valid, unexpired key unlocks all four.

The subject the key was issued to is not logged, since it is free-form and routinely an email address.

One line at startup states which mode the process is in: the plan and expiry when a key was accepted, or that the paid features are disabled when there is none.
A key that is set but does not verify logs a warning and leaves the paid features disabled; the server still starts, because refusing to would take the free half down over a paid-feature misconfiguration.

Verification is ordinary source code and can be removed by anyone who rebuilds.
That is deliberate: the protection is `ee/LICENSE`, under which using such a build is a licence violation.

## Email

Transactional email (invite notifications, billing alerts) does not exist: neither Stripe event handler sends any, and there is no environment variable to configure a provider (e.g. SES/Postmark).
A prior `EmailProvider` trait was removed since it had no real implementation and no caller; adding transactional email back requires both a provider implementation and wiring it into the handlers that would use it.
