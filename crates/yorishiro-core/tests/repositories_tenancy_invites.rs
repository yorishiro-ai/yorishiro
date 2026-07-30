use chrono::Duration;
use sqlx::PgPool;
use uuid::Uuid;

use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::tenancy::{
    MembershipRole, create_invite, create_tenant, redeem_invite,
};

#[sqlx::test(migrations = "../../migrations")]
async fn creates_and_redeems_an_invite(pool: PgPool) {
    let tenant = create_tenant(&pool, "team", None).await.unwrap();

    let (invite, token) = create_invite(
        &pool,
        tenant.id,
        "frank@example.com",
        MembershipRole::Member,
        Duration::hours(24),
    )
    .await
    .unwrap();
    assert_eq!(invite.tenant_id, tenant.id);
    assert_eq!(invite.email, "frank@example.com");
    assert_eq!(invite.role, MembershipRole::Member);

    let redeemed = redeem_invite(&pool, &token).await.unwrap().unwrap();
    assert_eq!(redeemed.id, invite.id);
    assert_eq!(redeemed.tenant_id, tenant.id);
    assert_eq!(redeemed.role, MembershipRole::Member);

    // A token can only be redeemed once.
    let second_attempt = redeem_invite(&pool, &token).await.unwrap();
    assert!(second_attempt.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn redeem_invite_rejects_unknown_or_garbled_tokens(pool: PgPool) {
    let result = redeem_invite(&pool, "not-a-real-token").await.unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn redeem_invite_rejects_an_expired_token(pool: PgPool) {
    let tenant = create_tenant(&pool, "team", None).await.unwrap();

    let (_invite, token) = create_invite(
        &pool,
        tenant.id,
        "grace@example.com",
        MembershipRole::Viewer,
        Duration::hours(-1),
    )
    .await
    .unwrap();

    let result = redeem_invite(&pool, &token).await.unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_invite_rejects_unknown_tenant(pool: PgPool) {
    let err = create_invite(
        &pool,
        Uuid::nil(),
        "nobody@example.com",
        MembershipRole::Member,
        Duration::hours(24),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}
