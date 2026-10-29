use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use loco_rs::task::Vars;
use yorishiro::App;
use yorishiro::models::tenant_tenants::retirement;

#[tokio::test]
async fn retirement_inventory_is_read_only_and_target_specific() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::_entities::{api_keys, workspace_workspaces};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let before = retirement::inventory(&ctx.db, tenant.tenant_id)
            .await
            .unwrap();
        let workspace_before = workspace_workspaces::Entity::find_by_id(tenant.workspace_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let keys_before = api_keys::Entity::find()
            .filter(api_keys::Column::WorkspaceId.eq(tenant.workspace_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(before.tenant_id, tenant.tenant_id);
        assert!(before.workspace_ids.contains(&tenant.workspace_id));
        assert!(before.memberships > 0);
        assert!(before.api_keys > 0);
        assert_eq!(before.identity_policy, "retain_all_user_accounts");
        let serialized = serde_json::to_value(&before).unwrap();
        assert!(!serialized.to_string().contains(&tenant.api_key));
        assert!(!serialized.to_string().contains("key_hash"));
        let vars = Vars::from_cli_args(vec![("tenant_id".into(), tenant.tenant_id.to_string())]);
        loco_rs::boot::run_task::<App>(&ctx, Some(&"inspect_tenant_retirement".to_owned()), &vars)
            .await
            .unwrap();
        let after = retirement::inventory(&ctx.db, tenant.tenant_id)
            .await
            .unwrap();
        assert_eq!(after.tenant_id, before.tenant_id);
        assert!(after.workspace_ids.contains(&tenant.workspace_id));
        assert_eq!(
            workspace_workspaces::Entity::find_by_id(tenant.workspace_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap(),
            workspace_before
        );
        let keys_after = api_keys::Entity::find()
            .filter(api_keys::Column::WorkspaceId.eq(tenant.workspace_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(keys_after).unwrap(),
            serde_json::to_value(keys_before).unwrap()
        );
        assert_eq!(
            request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .await
                .status_code(),
            200
        );
        assert!(
            retirement::inventory(&ctx.db, uuid::Uuid::nil())
                .await
                .is_err()
        );
        assert!(
            retirement::inventory(&ctx.db, uuid::Uuid::new_v4())
                .await
                .is_err()
        );
        assert!(
            loco_rs::boot::run_task::<App>(
                &ctx,
                Some(&"inspect_tenant_retirement".to_owned()),
                &Vars::default(),
            )
            .await
            .is_err()
        );
    })
    .await;
}
