use sqlx::PgPool;
use uuid::Uuid;

use crate::YorishiroError;
use crate::repositories::tenancy::{
    MembershipRole, add_member, create_tenant, create_user, get_membership_role, get_user_by_email,
    list_members,
};
use crate::services::auth::ApiKeyScope;

#[sqlx::test(migrations = "../../migrations")]
async fn adds_and_lists_members(pool: PgPool) {
    let tenant = create_tenant(&pool, "team", None).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = create_user(&mut conn, "carol@example.com", "pw", Some("Carol"))
        .await
        .unwrap();

    add_member(&mut conn, tenant.id, user.id, MembershipRole::Admin)
        .await
        .unwrap();

    let members = list_members(&pool, tenant.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, user.id);
    assert_eq!(members[0].role, MembershipRole::Admin);

    // Re-adding the same user updates the role instead of erroring.
    add_member(&mut conn, tenant.id, user.id, MembershipRole::Viewer)
        .await
        .unwrap();
    let members = list_members(&pool, tenant.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, MembershipRole::Viewer);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_membership_role_resolves_and_defaults_to_none(pool: PgPool) {
    let tenant = create_tenant(&pool, "team", None).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = create_user(&mut conn, "erin@example.com", "pw", None)
        .await
        .unwrap();

    assert_eq!(
        get_membership_role(&pool, tenant.id, user.id)
            .await
            .unwrap(),
        None
    );

    add_member(&mut conn, tenant.id, user.id, MembershipRole::Member)
        .await
        .unwrap();
    assert_eq!(
        get_membership_role(&pool, tenant.id, user.id)
            .await
            .unwrap(),
        Some(MembershipRole::Member)
    );
}

#[test]
fn max_scope_mirrors_role_privilege_order() {
    assert_eq!(MembershipRole::Owner.max_scope(), ApiKeyScope::Schema);
    assert_eq!(MembershipRole::Admin.max_scope(), ApiKeyScope::Schema);
    assert_eq!(MembershipRole::Member.max_scope(), ApiKeyScope::Write);
    assert_eq!(MembershipRole::Viewer.max_scope(), ApiKeyScope::Read);
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_member_rejects_unknown_tenant(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let user = create_user(&mut conn, "dave@example.com", "pw", None)
        .await
        .unwrap();
    let err = add_member(&mut conn, Uuid::nil(), user.id, MembershipRole::Member)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// The whole reason `create_user`/`add_member` take `&mut PgConnection` (rather than `&PgPool`,
/// like most of this module) is so a caller can compose them into one transaction -- see
/// `create_user`'s doc comment. This proves that composition actually prevents the bug it's
/// meant to prevent: if `add_member` fails partway through a transaction that already ran
/// `create_user`, rolling back the transaction must leave no orphaned user row behind (an
/// orphan would be a user nobody can ever add to a tenant -- signup expects the email not to
/// exist yet, `admin add-member` expects the user to already exist).
#[sqlx::test(migrations = "../../migrations")]
async fn create_user_and_add_member_roll_back_together_on_failure(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    create_user(&mut tx, "orphan-check@example.com", "pw", None)
        .await
        .unwrap();
    let err = add_member(&mut tx, Uuid::nil(), Uuid::nil(), MembershipRole::Member)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
    tx.rollback().await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let user = get_user_by_email(&mut conn, "orphan-check@example.com")
        .await
        .unwrap();
    assert!(
        user.is_none(),
        "create_user's row must not survive a rolled-back transaction"
    );
}
