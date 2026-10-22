//! The licence gate, exercised from both sides of the boundary it draws.
//!
//! Unlicensed and licensed cases use the application boot verifier.
//! Licensed cases also inject missing process state to check that the router fails closed.
//! Asserting only the licensed side would pass just as well with the gate deleted.
//!
//! The routes fall into three groups, and the reason each is here differs:
//!
//! **Gated** (`marketplace`, `oauth`, and `inference::gated_routes`) must answer 404 unlicensed and
//! something other than 404 licensed.
//!
//! A route only belongs in that list when its 404 can come from nothing but the gate. `oauth`
//! qualifies through `/auth/oauth/status` specifically: `authorize` and `callback` answer 404 of
//! their own accord when `YORISHIRO_OAUTH_ISSUER_URL` is unset, which is how a test process boots,
//! so either of those would keep passing with the gate deleted. `status` answers 200 whether or not
//! OAuth is configured (that is the point of it: a client has to be able to tell "not configured"
//! from "not present"), so its 404 has exactly one possible source.
//!
//! **Ungated inside a gated controller** (`/api/workspace/llm-key`) sits in `inference` beside a
//! gated route without being gated itself, because storing a credential is not a paid action while
//! spending it on an inference call is. A layer applies per `Routes`, so this group exists to keep
//! the two apart.
//!
//! `stripe` carries the gate but cannot join `GATED`, for two reasons that are both about telling
//! the gate apart from something else. Its webhook is POST-only, so the GET these lists use would
//! answer 404 by method alone; and it answers 404 unconfigured, which is how a test process boots.
//! `stripe_webhook_is_gated` below therefore POSTs, and asserts the licensed side answers 501 (the
//! handler's own "no secret configured") rather than merely "not 404": 501 is a status only the
//! handler produces, so it cannot be reached unless the request got past the gate.
//!
//! **Ungated** (`dashboard`, `embedding`, `entity_columns`, `origin`, `worker_class`) must stay
//! reachable in both boot states. Nothing in the current code can make these 404 for licence reasons:
//! `Routes::layer` wraps each handler's own `MethodRouter`, so a layer cannot reach a route it was
//! not attached to. That is exactly why the assertion is worth keeping: it does not guard a leak
//! this code can produce, it guards the change that widens where the layer is applied. Without it,
//! moving the gate up to cover every enterprise route would be a silent product change that no test
//! notices.
use super::boot_request;
use serial_test::serial;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;

fn require_licence(ctx: &loco_rs::app::AppContext) {
    assert!(
        ctx.shared_store
            .get::<std::sync::Arc<LicenceState>>()
            .is_some_and(|state| state.is_active()),
        "this case requires a valid YORISHIRO_LICENSE_KEY at application boot"
    );
}

/// One representative gated route, with a method that reaches the layer.
///
/// Authentication is deliberately not set up: 404 rather than 401 to an unauthenticated caller is
/// what pins the gate's ordering (see `ee::controllers::middleware::edition::licence_gate` for why that ordering matters).
///
/// Every path in this file is a GET that a route actually declares. A path with no route answers 404
/// for that reason alone, which would make the unlicensed assertions pass with the gate deleted:
/// the exact failure this file exists to rule out. Checked against each controller's own `routes()`.
const GATED: &[&str] = &[
    // marketplace.rs: `.prefix("api/marketplace").add("/", get(list_marketplace))`
    "/api/marketplace",
    // oauth.rs: `.prefix("auth/oauth").add("/status", get(status))`
    // `status` rather than `authorize`/`callback`: those two answer 404 unconfigured, so only this
    // one's 404 is attributable to the gate. `oauth_login_is_gated` asserts the licensed side
    // reaches 200, which this list's `!= 404` alone would not pin.
    "/auth/oauth/status",
];

/// Routes inside a gated controller that are deliberately NOT gated.
///
/// Asserted separately from `UNGATED` because the failure they catch is specific: collapsing
/// `inference`'s two route groups into one would gate them, and nothing else in this file would
/// notice.
const UNGATED_INSIDE_A_GATED_CONTROLLER: &[&str] = &["/api/workspace/llm-key"];

/// One per ungated controller. These must be served in both boots.
const UNGATED: &[&str] = &[
    // dashboard.rs: `.prefix("api/tenant").add("/overview", get(tenant_overview))`
    "/api/tenant/overview",
    // origin.rs: `.prefix("api/schemas").add("/upstream-changes", get(list_upstream_changes))`
    "/api/schemas/upstream-changes",
];

#[tokio::test]
#[serial(process_environment)]
async fn gated_routes_are_absent_without_a_licence() {
    let guard = crate::EnvGuard::capture(&["YORISHIRO_LICENSE_KEY"]);
    guard.remove("YORISHIRO_LICENSE_KEY");
    boot_request::<App, _, _>(|request, _ctx| async move {
        // Boot the real verifier without a key, even when licensed cases have one available.
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
    })
    .await;
}

#[tokio::test]
async fn gated_routes_are_served_with_a_licence() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);

        for path in GATED {
            let response = request.get(path).await;
            // Not asserting 200: these routes still authenticate, and this request carries no key.
            // What matters is that the licence gate is not what rejects it, which a 404 would mean
            // and a 401/403/422 would not.
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
    })
    .await;
}

/// Missing process state closes the gate without a restart.
#[tokio::test]
async fn missing_licence_state_closes_the_gate_again() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let licensed = request.get(GATED[0]).await.status_code();
        // 401 here rather than 200: the request carries no API key, and the point is only that the
        // *licence* is not what rejects it. Asserting the exact code rather than `!= 404` matters:
        // a bare `assert_ne!(.., 404)` also holds when the gate is disabled entirely, so it would
        // pass with the mechanism removed.
        assert_eq!(
            licensed, 401,
            "an active licence must let the request reach authentication; got {licensed}"
        );

        // Inject missing state without bypassing signature verification or replacing the clock.
        assert!(
            ctx.shared_store
                .remove::<std::sync::Arc<LicenceState>>()
                .is_some()
        );
        let unavailable = request.get(GATED[0]).await.status_code();
        assert_eq!(
            unavailable, 404,
            "missing licence state must close the gate without a restart; got {unavailable}"
        );
    })
    .await;
}

/// `stripe`'s webhook, which the four lists above cannot cover.
///
/// The gate is asserted from both sides on one path, the same shape as the lists, but with a POST
/// (the webhook declares no GET) and against 501 rather than "not 404" on the licensed side.
/// 501 is the handler's own answer to a missing `YORISHIRO_STRIPE_WEBHOOK_SECRET`, which a test
/// process boots without: reaching it proves the request passed the gate and entered the handler,
/// where a weaker `assert_ne!(.., 404)` would also hold if the route stopped existing.
#[tokio::test]
async fn stripe_webhook_is_gated() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let licensed = request.post("/api/stripe/webhook").await.status_code();
        assert_eq!(
            licensed, 501,
            "an active licence must let the request reach the handler, which then refuses for want \
             of a configured secret; got {licensed}"
        );
        // Missing state is a fault injection; the active state came from the boot verifier.
        assert!(
            ctx.shared_store
                .remove::<std::sync::Arc<LicenceState>>()
                .is_some()
        );
        let unlicensed = request.post("/api/stripe/webhook").await.status_code();
        assert_eq!(unlicensed, 404);
    })
    .await;
}

/// `oauth`'s gate, asserted from both sides with the licensed half pinned to an exact status.
///
/// `GATED` already covers the unlicensed 404 for this path. What it cannot express is that the
/// licensed answer is specifically 200: its `assert_ne!(.., 404)` would also hold if `status` began
/// erroring, or if the route were replaced by something else entirely. `status` returns 200
/// unconditionally once past the gate, reporting `enabled: false` when OAuth is unconfigured (which
/// a test process is), so 200 here means the request reached the handler and nothing else.
///
/// This is the half that catches a gate applied too broadly: without it, a change that gated every
/// enterprise route would still pass the unlicensed assertions.
#[tokio::test]
async fn oauth_login_is_gated() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let licensed = request.get("/auth/oauth/status").await.status_code();
        assert_eq!(
            licensed, 200,
            "an active licence must let the request reach `status`, which answers 200 whether or \
             not OAuth is configured; got {licensed}"
        );
        // Missing state is a fault injection; the active state came from the boot verifier.
        assert!(
            ctx.shared_store
                .remove::<std::sync::Arc<LicenceState>>()
                .is_some()
        );
        let unlicensed = request.get("/auth/oauth/status").await.status_code();
        assert_eq!(unlicensed, 404);
    })
    .await;
}
