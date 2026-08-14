use crate::http::controllers::oauth::{authorize, callback};
use crate::services::oauth;
use crate::state::HostedState;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;
use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::tenancy::{self, MembershipRole};

use crate::tests::test_helpers;
use test_helpers::hosted_state;

fn router(state: HostedState) -> Router {
    Router::new()
        .route("/auth/oauth/authorize", axum::routing::get(authorize))
        .route("/auth/oauth/callback", axum::routing::get(callback))
        .with_state(state)
}

/// With no `YORISHIRO_OAUTH_ISSUER_URL` configured, both OAuth routes must behave exactly as if
/// they didn't exist -- `404 Not Found` -- so a self-hosted/community-style deployment (or an
/// enterprise deployment that simply hasn't set up SSO) sees no difference from before this
/// feature existed.
#[sqlx::test(migrations = "../../../migrations")]
async fn authorize_404s_when_oauth_is_not_configured(pool: PgPool) {
    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/authorize")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../../migrations")]
async fn callback_404s_when_oauth_is_not_configured(pool: PgPool) {
    let app = router(hosted_state(pool));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/oauth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The provisioning logic behind the callback handler (`services::oauth::find_or_create`) is
/// exercised directly here rather than through the full HTTP callback, since a genuine callback
/// also requires talking to a live OIDC provider (discovery document, token exchange, JWKS) that
/// this test suite has no mock server for. This still covers the part specific to this feature:
/// auto-provisioning a brand-new tenant/workspace/`member`-role membership on first login, and
/// resolving back to the same account on a second login by the same provider+subject id.
#[sqlx::test(migrations = "../../../migrations")]
async fn find_or_create_provisions_a_tenant_and_membership_on_first_login(pool: PgPool) {
    let provisioned = oauth::find_or_create(
        &pool,
        "oidc",
        "subject-123",
        Some("alice@example.com"),
        Some("Alice"),
    )
    .await
    .unwrap();

    assert_eq!(provisioned.email, "alice@example.com");
    assert_eq!(provisioned.role, MembershipRole::Member);

    let workspace = tenancy::get_workspace(&pool, provisioned.workspace_id)
        .await
        .unwrap();
    let role = tenancy::get_membership_role(&pool, workspace.tenant_id, provisioned.user_id)
        .await
        .unwrap();
    assert_eq!(role, Some(MembershipRole::Member));
    // A tenant auto-provisioned through SSO has no Stripe subscription, so it's Free in every
    // way that matters -- the workspace it gets must carry Free's entity cap, not `None`
    // (unlimited), the same way it would if the tenant had been created through any other path.
    assert_eq!(
        workspace.max_entities,
        crate::services::plan::Plan::Free
            .caps()
            .default_max_entities
    );
}

#[sqlx::test(migrations = "../../../migrations")]
async fn find_or_create_resolves_the_same_user_on_a_second_login(pool: PgPool) {
    let first = oauth::find_or_create(
        &pool,
        "oidc",
        "subject-456",
        Some("bob@example.com"),
        Some("Bob"),
    )
    .await
    .unwrap();

    let second = oauth::find_or_create(&pool, "oidc", "subject-456", Some("bob@example.com"), None)
        .await
        .unwrap();

    assert_eq!(first.user_id, second.user_id);
    assert_eq!(first.workspace_id, second.workspace_id);
}

/// A different `subject_id` under the same `provider` must never resolve to an existing user --
/// otherwise two distinct identities at the provider could collide onto one Yorishiro account.
#[sqlx::test(migrations = "../../../migrations")]
async fn find_or_create_treats_distinct_subject_ids_as_distinct_users(pool: PgPool) {
    let first = oauth::find_or_create(
        &pool,
        "oidc",
        "subject-a",
        Some("shared-name@example.com"),
        None,
    )
    .await
    .unwrap();

    // A different subject id at the same provider, coincidentally sharing an email with an
    // account that isn't itself OAuth-provisioned under this identity, still fails loudly
    // (email already taken) rather than silently attaching to the wrong account.
    let second = oauth::find_or_create(
        &pool,
        "oidc",
        "subject-b",
        Some("shared-name@example.com"),
        None,
    )
    .await;

    assert!(
        first.user_id != uuid::Uuid::nil(),
        "sanity: first login succeeded"
    );
    // The two subject ids are genuinely distinct identities, so the advisory-lock-guarded
    // re-check in `find_or_create` (see `identity_lock_key`) finds no existing row for
    // `("oidc", "subject-b")` and this insert genuinely fails on the `email` unique constraint --
    // surfacing as `CreateOauthUserError::UniqueViolation` and then this Conflict error, not a
    // same-identity race.
    match second {
        Err(YorishiroError::Conflict { message }) => {
            assert!(message.contains("shared-name@example.com"));
        }
        Ok(_) => panic!("expected a Conflict error naming the duplicate email, got Ok"),
        Err(_) => panic!(
            "expected a Conflict error naming the duplicate email, got a different error variant"
        ),
    }
}

/// A concurrent first login for the exact same `(provider, subject_id)` is not a real conflict --
/// both requests are racing to provision the same identity's first login, so the second must
/// resolve to whatever the first created rather than fail. This exercises the
/// `pg_advisory_xact_lock` serialization in `find_or_create` (see `identity_lock_key`) by racing
/// two `find_or_create` calls for the same identity concurrently.
#[sqlx::test(migrations = "../../../migrations")]
async fn concurrent_first_logins_for_the_same_identity_both_resolve(pool: PgPool) {
    let (first, second) = tokio::join!(
        oauth::find_or_create(
            &pool,
            "oidc",
            "subject-race",
            Some("racer@example.com"),
            Some("Racer"),
        ),
        oauth::find_or_create(
            &pool,
            "oidc",
            "subject-race",
            Some("racer@example.com"),
            Some("Racer"),
        ),
    );

    let first = first.expect("first concurrent login should succeed");
    let second = second.expect("second concurrent login should resolve, not fail");
    assert_eq!(first.user_id, second.user_id);
    assert_eq!(first.workspace_id, second.workspace_id);
}

/// A provider that omits the `email` claim entirely can't be auto-provisioned -- there is no
/// email to create the account or tenant name from.
#[sqlx::test(migrations = "../../../migrations")]
async fn find_or_create_rejects_a_new_identity_with_no_email_claim(pool: PgPool) {
    let result = oauth::find_or_create(&pool, "oidc", "subject-no-email", None, None).await;
    assert!(result.is_err());
}

/// `find_or_create` runs `oauth::create_oauth_user` and `tenancy::add_member` on the same
/// transaction (see their doc comments) so a failure between them rolls back the user-row insert
/// instead of leaving an orphaned user with no tenant membership. Exercised directly at this
/// level -- rather than by forcing `find_or_create` itself to fail partway through -- because
/// `find_or_create`'s own tenant/workspace it creates always exists, so nothing inside it would
/// ever hit `add_member`'s "tenant not found" path; a bogus tenant id is the simplest way to make
/// `add_member` fail without special-casing a real caller. Mirrors base's
/// `create_user_and_add_member_roll_back_together_on_failure`.
#[sqlx::test(migrations = "../../../migrations")]
async fn create_oauth_user_and_add_member_roll_back_together_on_failure(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    let user = oauth::create_oauth_user(
        &mut tx,
        "orphan@example.com",
        None,
        "oidc",
        "subject-rollback",
    )
    .await
    .expect("insert should succeed inside the transaction");

    // A tenant id nothing points to: `add_member`'s own `get_tenant` call fails, so this must
    // fail before the membership insert -- exactly the "insert, then fail" shape a mid-flight
    // crash would produce if these two writes weren't sharing a transaction.
    let add_member_result = tenancy::add_member(
        &mut tx,
        uuid::Uuid::new_v4(),
        user.id,
        MembershipRole::Member,
    )
    .await;
    assert!(
        add_member_result.is_err(),
        "sanity: add_member must fail against a tenant id that doesn't exist"
    );

    tx.rollback().await.unwrap();

    let found = oauth::find_or_create(&pool, "oidc", "subject-rollback", None, None).await;
    // No email claim would normally be a `ValidationFailed`, before any lookup happens -- so
    // getting anything else here means the lookup itself ran and is what's being asserted: were
    // the rollback incomplete, `find_by_oauth_identity` would find the orphaned row from above
    // and this would resolve through `resolve_existing_login` instead of failing on the missing
    // email claim.
    assert!(
        matches!(found, Err(YorishiroError::ValidationFailed { .. })),
        "the user row from the rolled-back transaction must not be visible: got {found:?}"
    );
}
