#![cfg(feature = "enterprise")]

use loco_rs::task::Vars;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use yorishiro::App;
use yorishiro::models::_entities::tenant_memberships::Column as MembershipColumn;
use yorishiro::models::_entities::tenant_retirement_events::Column;
use yorishiro::models::{tenant_retirement_events, tenant_tenants};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

#[tokio::test]
async fn populated_tenant_cleanup_removes_schema_forks_entities_and_templates() {
    if !crate::require_postgres_backend() {
        return;
    }
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use tenant_tenants::retirement;
    use yorishiro::models::_entities::{
        entity_entities, schema_schemas, template_templates, workspace_schema_fork_heads,
        workspace_schema_forks,
    };
    use yorishiro::models::api_keys::ApiKeyScope;

    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs { key_scope: ApiKeyScope::Schema, ..Default::default() }).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs { key_scope: ApiKeyScope::Schema, ..Default::default() }).await;
        let created = request.post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({"name":"retirement-notes", "definition":yorishiro::data::templates::get_template("general-notes").unwrap()})).await;
        assert_eq!(created.status_code(), StatusCode::CREATED, "{}", created.text());
        let template_id: uuid::Uuid = created.json::<Value>()["id"].as_str().unwrap().parse().unwrap();
        let mut schemas = Vec::new();
        for tenant in [&target, &other] {
            let created = request.post("/api/schemas")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .json(&json!({"template_id":"task-management"})).await;
            assert_eq!(created.status_code(), StatusCode::CREATED, "{}", created.text());
            schemas.push(created.json::<Value>()["schema"]["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap());
        }
        let workspace = request.post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({"name":format!("retirement-fork-{}", uuid::Uuid::new_v4())})).await;
        assert_eq!(workspace.status_code(), StatusCode::CREATED);
        let workspace_id = workspace.json::<Value>()["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();
        let fork_key = crate::requests::fixtures::issue_api_key(&ctx, workspace_id, target.owner_id, ApiKeyScope::Schema, false).await;
        let fork = request.post("/api/schema-forks").add_header("Authorization", format!("Bearer {fork_key}"))
            .json(&json!({"source_workspace_id":target.workspace_id,"source_schema_id":schemas[0]})).await;
        assert_eq!(fork.status_code(), StatusCode::CREATED, "{}", fork.text());
        let fork_id = fork.json::<Value>()["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();
        let entity = request.post("/api/entities")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .json(&json!({"entity_type":"task", "data":{"title":"retire me"}})).await;
        assert_eq!(entity.status_code(), StatusCode::CREATED, "{}", entity.text());
        let entity_id = entity.json::<Value>()["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();
        retirement::disable_by_operator(&ctx.db, target.tenant_id, target.tenant_id).await.unwrap();
        retirement::delete_by_operator(&ctx.db, target.tenant_id, target.tenant_id).await.unwrap();
        assert!(template_templates::Entity::find_by_id(template_id).one(&ctx.db).await.unwrap().is_none());
        assert!(schema_schemas::Entity::find_by_id(schemas[0]).one(&ctx.db).await.unwrap().is_none());
        assert!(entity_entities::Entity::find_by_id(entity_id).one(&ctx.db).await.unwrap().is_none());
        assert!(workspace_schema_forks::Entity::find_by_id(fork_id).one(&ctx.db).await.unwrap().is_none());
        assert!(workspace_schema_fork_heads::Entity::find().filter(workspace_schema_fork_heads::Column::ForkId.eq(fork_id)).one(&ctx.db).await.unwrap().is_none());
        assert!(schema_schemas::Entity::find_by_id(schemas[1]).one(&ctx.db).await.unwrap().is_some());
        request.get("/api/whoami").add_header("Authorization", format!("Bearer {}", other.api_key))
            .await.assert_status(StatusCode::OK);
    }).await;
}

#[tokio::test]
async fn deletion_audit_failure_preserves_disabled_tenant_and_allows_retry() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::ConnectionTrait;
    use tenant_tenants::retirement;
    use yorishiro::models::workspace_workspaces;

    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        retirement::disable_by_operator(&ctx.db, target.tenant_id, target.tenant_id).await.unwrap();
        let before = retirement::inventory(&ctx.db, target.tenant_id).await.unwrap();
        // Fail the last write without interfering with the already-committed disable event.
        ctx.db.execute_unprepared("ALTER TABLE tenant_retirement_events ADD CONSTRAINT reject_deleted_event CHECK (action <> 'deleted')")
            .await.unwrap();
        let error = retirement::delete_by_operator(&ctx.db, target.tenant_id, target.tenant_id).await.unwrap_err();
        assert!(error.to_string().contains("reject_deleted_event"), "{error}");
        assert!(tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id).await.unwrap().disabled_at.is_some());
        assert!(workspace_workspaces::Entity::find_by_id(target.workspace_id).one(&ctx.db).await.unwrap().is_some());
        let after = retirement::inventory(&ctx.db, target.tenant_id).await.unwrap();
        assert_eq!(serde_json::to_value(&after).unwrap(), serde_json::to_value(&before).unwrap());
        let events = tenant_retirement_events::Entity::find().filter(Column::TenantId.eq(target.tenant_id))
            .all(&ctx.db).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "disabled");
        request.get("/api/whoami").add_header("Authorization", format!("Bearer {}", target.api_key))
            .await.assert_status(axum::http::StatusCode::UNAUTHORIZED);
        ctx.db.execute_unprepared("ALTER TABLE tenant_retirement_events DROP CONSTRAINT reject_deleted_event")
            .await.unwrap();
        retirement::delete_by_operator(&ctx.db, target.tenant_id, target.tenant_id).await.unwrap();
        assert!(tenant_tenants::Entity::find_by_id(target.tenant_id).one(&ctx.db).await.unwrap().is_none());
        let events = tenant_retirement_events::Entity::find().filter(Column::TenantId.eq(target.tenant_id))
            .all(&ctx.db).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events.iter().filter(|event| event.action == "deleted").count(), 1);
    }).await;
}

#[tokio::test]
async fn irreversible_cleanup_requires_disablement_and_preserves_shared_identities() {
    if !crate::require_postgres_backend() {
        return;
    }
    use tenant_tenants::retirement;
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::{tenant_memberships, user_users, workspace_workspaces};
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        tenant_memberships::add_member(
            &ctx.db,
            other.tenant_id,
            target.owner_id,
            MembershipRole::Viewer,
        )
        .await
        .unwrap();
        assert!(
            retirement::delete_by_operator(&ctx.db, target.tenant_id, other.tenant_id)
                .await
                .is_err()
        );
        assert!(matches!(
            retirement::delete_by_operator(&ctx.db, target.tenant_id, target.tenant_id).await,
            Err(yorishiro::YorishiroError::Conflict { .. })
        ));
        retirement::disable_by_operator(&ctx.db, target.tenant_id, target.tenant_id)
            .await
            .unwrap();
        retirement::delete_by_operator(&ctx.db, target.tenant_id, target.tenant_id)
            .await
            .unwrap();
        retirement::delete_by_operator(&ctx.db, target.tenant_id, target.tenant_id)
            .await
            .unwrap();
        assert!(
            tenant_tenants::Entity::find_by_id(target.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            workspace_workspaces::Entity::find_by_id(target.workspace_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            user_users::Entity::find_by_id(target.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            tenant_memberships::Entity::find()
                .filter(MembershipColumn::TenantId.eq(other.tenant_id))
                .filter(MembershipColumn::UserId.eq(target.owner_id))
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .role,
            "viewer"
        );
        let events = tenant_retirement_events::Entity::find()
            .filter(Column::TenantId.eq(target.tenant_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.action == "deleted")
                .count(),
            1
        );
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
        let missing = uuid::Uuid::new_v4();
        assert!(
            retirement::delete_by_operator(&ctx.db, missing, missing)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn operator_disablement_requires_confirmation_and_is_retry_safe() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let task = Some("disable_tenant".to_owned());
        for args in [
            vec![("tenant_id".into(), target.tenant_id.to_string())],
            vec![
                ("tenant_id".into(), target.tenant_id.to_string()),
                ("confirmed_tenant_id".into(), other.tenant_id.to_string()),
            ],
        ] {
            assert!(
                loco_rs::boot::run_task::<App>(&ctx, task.as_ref(), &Vars::from_cli_args(args))
                    .await
                    .is_err()
            );
        }
        assert!(
            tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
                .await
                .unwrap()
                .disabled_at
                .is_none()
        );
        let vars = Vars::from_cli_args(vec![
            ("tenant_id".into(), target.tenant_id.to_string()),
            ("confirmed_tenant_id".into(), target.tenant_id.to_string()),
        ]);
        loco_rs::boot::run_task::<App>(&ctx, task.as_ref(), &vars)
            .await
            .unwrap();
        let disabled_at = tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
            .await
            .unwrap()
            .disabled_at;
        assert!(disabled_at.is_some());
        loco_rs::boot::run_task::<App>(&ctx, task.as_ref(), &vars)
            .await
            .unwrap();
        assert_eq!(
            tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
                .await
                .unwrap()
                .disabled_at,
            disabled_at
        );
        let events = tenant_retirement_events::Entity::find()
            .filter(Column::TenantId.eq(target.tenant_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "disabled");
        assert_eq!(events[0].actor_user_id, None);
        let inventory = tenant_tenants::retirement::inventory(&ctx.db, target.tenant_id)
            .await
            .unwrap();
        assert_eq!(inventory.api_keys, 0);
        assert!(inventory.workspace_ids.contains(&target.workspace_id));
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .await
            .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
    })
    .await;
}
