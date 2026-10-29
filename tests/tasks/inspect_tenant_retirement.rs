use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use loco_rs::task::Vars;
use yorishiro::App;
use yorishiro::models::tenant_tenants::retirement;

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn retirement_inventory_counts_shared_accounts_without_exposing_them() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let first = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let second = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            second.tenant_id,
            first.owner_id,
            yorishiro::models::tenant_memberships::MembershipRole::Viewer,
        )
        .await
        .unwrap();
        for tenant in [&first, &second] {
            let inventory = retirement::inventory(&ctx.db, tenant.tenant_id)
                .await
                .unwrap();
            assert_eq!(inventory.shared_user_accounts, 1);
            assert_eq!(inventory.external_workspace_forks, 0);
            let body = serde_json::to_value(inventory).unwrap();
            assert!(!body.to_string().contains(&first.owner_id.to_string()));
            assert!(!body.to_string().contains(&first.api_key));
        }
    })
    .await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn retirement_inventory_detects_external_workspace_fork_dependencies() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    use yorishiro::models::{schema_schemas, workspace_schema_forks};

    boot_request::<App, _, _>(|request, ctx| async move {
        let source = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let definition = yorishiro::data::templates::get_template("general-notes").unwrap();
        let (source_schema, _) = schema_schemas::create_schema(
            &ctx.db,
            source.tenant_id,
            source.workspace_id,
            definition.clone(),
            None,
            None,
        )
        .await
        .unwrap();
        let (target_schema, _) = schema_schemas::create_schema(
            &ctx.db,
            target.tenant_id,
            target.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .unwrap();
        // The API forbids cross-tenant forks, but a persisted reference must still block cleanup.
        workspace_schema_forks::ActiveModel {
            tenant_id: Set(target.tenant_id),
            workspace_id: Set(target.workspace_id),
            source_workspace_id: Set(source.workspace_id),
            source_schema_id: Set(source_schema.id),
            source_schema_version: Set(source_schema.version),
            source_schema_name: Set(source_schema.name),
            fork_schema_id: Set(target_schema.id),
            customized: Set(false),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let inventory = retirement::inventory(&ctx.db, source.tenant_id)
            .await
            .unwrap();
        assert_eq!(inventory.external_workspace_forks, 1);
        let inventory = retirement::inventory(&ctx.db, target.tenant_id)
            .await
            .unwrap();
        assert_eq!(inventory.external_workspace_forks, 0);
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", source.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
    })
    .await;
}

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
