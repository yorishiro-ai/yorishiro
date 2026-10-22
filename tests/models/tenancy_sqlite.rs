/// SQLite-specific tests for tenancy: single-tenant cap and invite ID generation.
use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::tenant_memberships::MembershipRole;
use yorishiro::models::tenant_tenants::create_tenant;
use yorishiro::models::workspace_invites::create_invite;

#[tokio::test]
async fn a_first_tenant_can_be_created_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let args = TenantArgs::default();
        let name = args.tenant_name.clone();
        let tenant = TestTenant::create(&request, &ctx, args).await;
        let stored =
            yorishiro::models::tenant_tenants::Model::find_by_id(&ctx.db, tenant.tenant_id)
                .await
                .expect("created tenant");
        assert_eq!(stored.name, name);
    })
    .await;
}

#[tokio::test]
async fn a_second_tenant_is_refused_on_sqlite_even_with_a_large_max_tenants() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let err = create_tenant(&ctx.db, "second tenant", Some(1000))
            .await
            .expect_err("a second tenant must be refused on sqlite");
        assert!(
            matches!(err, YorishiroError::Conflict { .. }),
            "expected Conflict, got {err:?}"
        );
    })
    .await;
}

/// `create_invite` builds its `ActiveModel` with `..Default::default()`, so `id` reaches the insert `NotSet`.
/// PostgreSQL fills it from the column's `uuidv7()` default; SQLite has no such default, so this table's `before_save` must call `db::sqlite_generated_id` or the insert fails with `NOT NULL constraint failed: workspace_invites.id`.
///
/// Keep this backend-specific assertion even though the common fixture also exercises SQLite ID generation.
#[tokio::test]
async fn an_invite_gets_an_id_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let (invite, _token) = create_invite(
            &ctx.db,
            tenant.tenant_id,
            &crate::requests::fixtures::unique_email("invitee"),
            MembershipRole::Member,
            chrono::Duration::days(7),
        )
        .await
        .expect("create invite on sqlite");

        assert!(
            !invite.id.is_nil(),
            "the invite must carry a generated id, not a nil UUID"
        );
    })
    .await;
}

#[tokio::test]
async fn invite_expiry_boundaries_match_sqlite_gt() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        super::tenancy::assert_invitation_boundaries(&ctx.db, tenant.tenant_id).await;
    })
    .await;
}
