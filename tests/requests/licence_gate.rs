//! The licence gate, exercised from both sides of the boundary it draws.
//!
//! A gate is not a gate until a deliberate violation makes it fire, so these boot the whole
//! application twice — once with no licence and once with a valid one — and compare what the router
//! actually serves. Asserting only the licensed side would pass just as well with the gate deleted.
//!
//! The routes fall into four groups, and the reason each is here differs:
//!
//! **Gated** (`marketplace`, and `inference::gated_routes`) must answer 404 unlicensed and something
//! other than 404 licensed. These are exactly the surfaces that carried `LicenceState::require_active`
//! in their own handlers before the gate became a layer.
//!
//! **Ungated inside a gated controller** (`/hosted/workspace/llm-key`) is the group that exists
//! because a layer applies per `Routes` while the check it replaced was written per handler. These
//! three sit in `inference` beside a gated route and were never gated themselves.
//!
//! **Config-gated** (`oauth`, `stripe`) are deliberately absent from these assertions. Both already
//! answer 404 when their own configuration is unset, which is the state a test process boots in, so
//! "404 without a licence" would hold for them whether or not a licence gate existed at all — the
//! assertion would keep passing with the mechanism it claims to check deleted.
//!
//! **Ungated** (`dashboard`, `embedding`, `entity_columns`, `origin`, `worker_class`) must stay
//! reachable in *both* boots. Nothing in the current code can make these 404 for licence reasons:
//! `Routes::layer` wraps each handler's own `MethodRouter`, so a layer cannot reach a route it was
//! not attached to. That is exactly why the assertion is worth keeping — it does not guard a leak
//! this code can produce, it guards the change that widens where the layer is applied. Without it,
//! moving the gate up to cover every paid route would be a silent product change that no test
//! notices.
use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::ee::services::licence::{LicenceClaims, LicenceState};

use super::close_app_pools;

/// Overwrites the `LicenceState::from_env()` the test process booted with.
///
/// `shared_store.insert` is keyed by `TypeId`, so the later insert wins, and the gate reads the
/// state per request rather than capturing it at boot — which is the property that makes this
/// possible at all, and the same one that lets a key expiring mid-process take effect without a
/// restart.
fn install_licence(ctx: &loco_rs::app::AppContext, expires_in_secs: i64) {
    ctx.shared_store
        .insert(LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: chrono::Utc::now().timestamp() + expires_in_secs,
        }));
}

/// One representative gated route, with a method that reaches the layer.
///
/// Authentication is deliberately not set up: the gate runs before the handler, so an unlicensed
/// deployment answers 404 to an unauthenticated caller for the licence reason rather than 401 for
/// the authentication one. That ordering is the anti-probing property the layer inherits from the
/// original in-handler checks, and asserting it here is what pins it.
///
/// Every path in this file is a GET that a route actually declares. A path with no route answers 404
/// for that reason alone, which would make the unlicensed assertions pass with the gate deleted —
/// the exact failure this file exists to rule out. Checked against each controller's own `routes()`.
const GATED: &[&str] = &[
    // marketplace.rs: `.prefix("hosted").add("/marketplace", get(list_marketplace))`
    "/hosted/marketplace",
];

/// Routes inside a gated controller that are deliberately NOT gated.
///
/// `inference` declares two route groups for this reason: the licence check it replaces was written
/// per handler, and these three never called it. Storing an LLM key has never needed a licence; only
/// spending it on an inference call has. They are asserted separately from `UNGATED` because the
/// failure they catch is specific — collapsing `inference`'s two groups back into one would gate
/// them, and nothing else in this file would notice.
const UNGATED_INSIDE_A_GATED_CONTROLLER: &[&str] = &["/hosted/workspace/llm-key"];

/// One per ungated controller. These must be served in both boots.
const UNGATED: &[&str] = &[
    // dashboard.rs: `.prefix("hosted").add("/tenant/overview", get(tenant_overview))`
    "/hosted/tenant/overview",
    // origin.rs: `.prefix("api/schemas").add("/upstream-changes", get(list_upstream_changes))`
    "/api/schemas/upstream-changes",
];

#[tokio::test]
#[serial]
async fn gated_routes_are_absent_without_a_licence() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        // No licence installed: the process booted with whatever `from_env` found, which in a test
        // environment is nothing.
        for path in GATED {
            let response = request.get(path).await;
            assert_eq!(
                response.status_code(),
                404,
                "{path} must answer 404 with no licence; got {}",
                response.status_code()
            );
        }

        // The half that catches a gate applied too broadly.
        for path in UNGATED {
            let response = request.get(path).await;
            assert_ne!(
                response.status_code(),
                404,
                "{path} carries no licence gate and must stay reachable without a licence"
            );
        }

        // The same, one level finer: these sit inside a controller that IS gated, and must still
        // answer without a licence.
        for path in UNGATED_INSIDE_A_GATED_CONTROLLER {
            let response = request.get(path).await;
            assert_ne!(
                response.status_code(),
                404,
                "{path} shares a controller with a gated route but is not itself gated; it must \
                 stay reachable without a licence"
            );
        }

        close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn gated_routes_are_served_with_a_licence() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        install_licence(&ctx, 60 * 60);

        for path in GATED {
            let response = request.get(path).await;
            // Not asserting 200: these routes still authenticate, and this request carries no key.
            // What matters is that the licence gate is no longer the thing rejecting it, which a
            // 404 would mean and a 401/403/422 would not.
            assert_ne!(
                response.status_code(),
                404,
                "{path} must be served once a licence is active; still 404"
            );
        }

        for path in UNGATED {
            let response = request.get(path).await;
            assert_ne!(
                response.status_code(),
                404,
                "{path} must stay reachable with a licence too"
            );
        }

        close_app_pools(&ctx).await;
    })
    .await;
}

/// The reason the gate is a per-request layer rather than a boot-time route set.
///
/// A licence whose `exp` has already passed is not merely absent — it verified once and then
/// lapsed. `LicenceState::is_active` compares against the current clock on every call, so the gate
/// closes again with no restart. A conditional `add_route` decided at boot could not express this:
/// the route would stay mounted for the life of the process.
#[tokio::test]
#[serial]
async fn an_expired_licence_closes_the_gate_again() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        // Unlicensed first, so the 404 below is known to be reachable on this path at all. Without
        // this the test would pass against a gate that never opens.
        let unlicensed = request.get(GATED[0]).await.status_code();
        assert_eq!(
            unlicensed, 404,
            "precondition: no licence must close the gate; got {unlicensed}"
        );

        install_licence(&ctx, 60 * 60);
        let licensed = request.get(GATED[0]).await.status_code();
        // 401 here rather than 200: the request carries no API key, and the point is only that the
        // *licence* is no longer what rejects it. Asserting the exact code rather than `!= 404`
        // matters — a bare `assert_ne!(.., 404)` also holds when the gate is disabled entirely, so
        // it would pass with the mechanism removed (confirmed by running exactly that).
        assert_eq!(
            licensed, 401,
            "an active licence must let the request reach authentication; got {licensed}"
        );

        // Same shape of state, expiry in the past. Nothing is restarted between these two calls.
        install_licence(&ctx, -1);
        let expired = request.get(GATED[0]).await.status_code();
        assert_eq!(
            expired, 404,
            "an expired licence must close the gate again without a restart; got {expired}"
        );

        close_app_pools(&ctx).await;
    })
    .await;
}
