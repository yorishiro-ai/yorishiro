use sqlx::PgPool;
use uuid::Uuid;

use yorishiro_core::YorishiroError;
use yorishiro_core::repositories::tenancy::{
    MembershipRole, add_member, create_tenant, create_user, get_membership_role, list_members,
};
use yorishiro_core::services::auth::ApiKeyScope;

#[sqlx::test(migrations = "../../migrations")]
async fn adds_and_lists_members(pool: PgPool) {
    let tenant = create_tenant(&pool, "team", None).await.unwrap();
    let user = create_user(&pool, "carol@example.com", "pw", Some("Carol"))
        .await
        .unwrap();

    add_member(&pool, tenant.id, user.id, MembershipRole::Admin)
        .await
        .unwrap();

    let members = list_members(&pool, tenant.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, user.id);
    assert_eq!(members[0].role, MembershipRole::Admin);

    // Re-adding the same user updates the role instead of erroring.
    add_member(&pool, tenant.id, user.id, MembershipRole::Viewer)
        .await
        .unwrap();
    let members = list_members(&pool, tenant.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, MembershipRole::Viewer);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_membership_role_resolves_and_defaults_to_none(pool: PgPool) {
    let tenant = create_tenant(&pool, "team", None).await.unwrap();
    let user = create_user(&pool, "erin@example.com", "pw", None)
        .await
        .unwrap();

    assert_eq!(
        get_membership_role(&pool, tenant.id, user.id)
            .await
            .unwrap(),
        None
    );

    add_member(&pool, tenant.id, user.id, MembershipRole::Member)
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
    let user = create_user(&pool, "dave@example.com", "pw", None)
        .await
        .unwrap();
    let err = add_member(&pool, Uuid::nil(), user.id, MembershipRole::Member)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}
