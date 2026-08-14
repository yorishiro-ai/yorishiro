# Deploying the paid features

**English** | [日本語](ja/deployment.md)

Deployment itself is in [the main deployment guide](../../docs/deployment.md): one binary, one image, one systemd unit, one release.
This page covers only what changes when the paid features under `ee/` are switched on.

## Turning them on

Set `YORISHIRO_LICENSE_KEY`.
Without it the binary runs the free half and answers `404` on the four gated surfaces — see [configuration.md](configuration.md#licence-keys) for the full list and for what an invalid or expired key does.

The startup log states which mode the process is in.
Check that line first when a paid feature is unexpectedly absent: it distinguishes a key that was rejected from one that was never set.

## Multi-tenant deployments

A hosted deployment sets `YORISHIRO_MAX_TENANTS=0`, which removes the single-tenant cap.
That also disables the first-run setup wizard, deliberately: with no cap, anyone reaching `POST /setup` between a deploy and its first tenant could claim the deployment.
Tenants arrive through Stripe checkout or invite redemption instead.

A self-hosted deployment leaves the variable alone.
The default is `1`, the wizard is enabled, and any paid features its licence covers work the same way.

## Onboarding a tenant

Tenant creation and the initial owner account work as [the setup guide](../../docs/setup.md) describes: the admin CLI, or `POST /auth/signup` redeeming an invite.

With `YORISHIRO_OAUTH_ISSUER_URL` configured (see [configuration.md](configuration.md#oauth2oidc-login)), a tenant can also onboard itself.
The first person from an organization to sign in with SSO gets a tenant, workspace and `member`-role membership provisioned on the spot, no invite needed.
Every subsequent teammate needs an invite from that first member, exactly as password-based signup does — auto-provisioning fires only for an identity provider `sub` this deployment has never seen.

A tenant has no `plan` and no `max_workspaces` cap until Stripe reports a subscription for it: `checkout.session.completed` links the Stripe customer, then `customer.subscription.created`/`updated` applies the plan.
See [api.md](api.md#post-hostedstripewebhook).

## Exposing the Stripe webhook

`POST /hosted/stripe/webhook` must be reachable from Stripe, and must keep its raw request body intact — the signature is computed over the exact bytes.
A proxy that re-encodes or pretty-prints JSON breaks verification for every event.

The endpoint is never rate-limited, on purpose: dropping a legitimate billing event on a `429` is worse than not rate-limiting a signature-verified request that Stripe itself makes.
It does keep the body-size cap, since an unbounded webhook body is its own denial-of-service vector.

## OAuth behind a proxy

Set `YORISHIRO_OAUTH_REDIRECT_URI` explicitly to the public `https://` URL.
The default derives from the bind address, which a browser cannot reach when the server sits behind a proxy.

The CSRF cookie's `Secure` attribute follows that URI's scheme, so an `https://` redirect URI is both required for the provider to reach the callback and sufficient to get the stricter cookie.
