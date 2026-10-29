use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use loco_rs::task::Vars;
use yorishiro::App;
use yorishiro::models::tenant_tenants::retirement;

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn retirement_audit_failure_rolls_back_disablement_and_allows_retry() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_retirement_events::Column as EventColumn;
    use yorishiro::models::{
        api_keys, query_embedding_requests as queries, queue_job_lifecycles as jobs,
        tenant_retirement_events, tenant_tenants,
    };

    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = api_keys::Entity::create_api_key(
            &ctx.db, target.workspace_id, api_keys::ApiKeyScope::Read, Some(target.owner_id), false,
        ).await.unwrap();
        let actor = api_keys::AuthContext {
            api_key_id: key.id, workspace_id: target.workspace_id, tenant_id: target.tenant_id,
            user_id: Some(target.owner_id), scope: api_keys::ApiKeyScope::Read, audit: false,
        };
        let job_id = uuid::Uuid::now_v7();
        jobs::Entity::record_enqueue(&ctx.db, jobs::Enqueue {
            id: job_id, job_name: "retirement-rollback", worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            workspace_id: Some(target.workspace_id), plan: None, concurrency_key: None, concurrency_limit: None,
        }).await.unwrap();
        let query_id = queries::Entity::open(&ctx.db, target.workspace_id, "retirement rollback", 60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1)).await.unwrap();
        let before = retirement::inventory(&ctx.db, target.tenant_id).await.unwrap();
        // Isolated DDL makes the final audit write fail after the earlier mutations execute.
        ctx.db.execute_unprepared("ALTER TABLE tenant_retirement_events ADD CONSTRAINT reject_retirement_audit CHECK (false)")
            .await.unwrap();
        let error = retirement::disable_as(&ctx.db, &actor, target.tenant_id).await.unwrap_err();
        assert!(error.to_string().contains("reject_retirement_audit"), "{error}");
        assert!(tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id).await.unwrap().disabled_at.is_none());
        assert_eq!(retirement::inventory(&ctx.db, target.tenant_id).await.unwrap().api_keys, before.api_keys);
        assert_eq!(jobs::Entity::find_by_id(job_id).one(&ctx.db).await.unwrap().unwrap().status, "queued");
        assert_eq!(queries::Entity::find_by_id(query_id).one(&ctx.db).await.unwrap().unwrap().status, "pending");
        assert_eq!(tenant_retirement_events::Entity::find().filter(EventColumn::TenantId.eq(target.tenant_id))
            .count(&ctx.db).await.unwrap(), 0);
        request.get("/api/whoami").add_header("Authorization", format!("Bearer {}", key.plaintext))
            .await.assert_status(axum::http::StatusCode::OK);
        ctx.db.execute_unprepared("ALTER TABLE tenant_retirement_events DROP CONSTRAINT reject_retirement_audit")
            .await.unwrap();
        retirement::disable_as(&ctx.db, &actor, target.tenant_id).await.unwrap();
        assert_eq!(jobs::Entity::find_by_id(job_id).one(&ctx.db).await.unwrap().unwrap().status, "cancelled");
        assert_eq!(queries::Entity::find_by_id(query_id).one(&ctx.db).await.unwrap().unwrap().status, "failed");
        assert_eq!(retirement::inventory(&ctx.db, target.tenant_id).await.unwrap().api_keys, 0);
        assert_eq!(tenant_retirement_events::Entity::find().filter(EventColumn::TenantId.eq(target.tenant_id))
            .count(&ctx.db).await.unwrap(), 1);
    }).await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn confirmed_owner_disablement_revokes_keys_and_retains_data_and_evidence() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_retirement_events::Column as RetirementEventColumn;
    use yorishiro::models::tenant_memberships::MembershipRole;
    use yorishiro::models::{
        api_keys, tenant_memberships, tenant_retirement_events, tenant_tenants,
    };

    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            target.workspace_id,
            api_keys::ApiKeyScope::Read,
            Some(target.owner_id),
            false,
        )
        .await
        .unwrap();
        let actor = api_keys::AuthContext {
            api_key_id: key.id,
            workspace_id: target.workspace_id,
            tenant_id: target.tenant_id,
            user_id: Some(target.owner_id),
            scope: api_keys::ApiKeyScope::Read,
            audit: false,
        };
        tenant_memberships::add_member(
            &ctx.db,
            target.tenant_id,
            other.owner_id,
            MembershipRole::Owner,
        )
        .await
        .unwrap();
        let pending = yorishiro::models::query_embedding_requests::Entity::open(
            &ctx.db,
            target.workspace_id,
            "retirement query",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        let before = retirement::inventory(&ctx.db, target.tenant_id)
            .await
            .unwrap();
        assert!(
            retirement::disable_as(&ctx.db, &actor, other.tenant_id)
                .await
                .is_err()
        );
        let service = api_keys::Entity::create_api_key(
            &ctx.db,
            target.workspace_id,
            api_keys::ApiKeyScope::Migration,
            None,
            false,
        )
        .await
        .unwrap();
        let service_actor = api_keys::AuthContext {
            api_key_id: service.id,
            user_id: None,
            ..actor.clone()
        };
        assert!(
            retirement::disable_as(&ctx.db, &service_actor, target.tenant_id)
                .await
                .is_err()
        );
        tenant_memberships::add_member(
            &ctx.db,
            target.tenant_id,
            target.owner_id,
            MembershipRole::Admin,
        )
        .await
        .unwrap();
        assert!(matches!(
            retirement::disable_as(&ctx.db, &actor, target.tenant_id).await,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        assert!(
            tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
                .await
                .unwrap()
                .disabled_at
                .is_none()
        );
        assert_eq!(
            tenant_retirement_events::Entity::find()
                .filter(RetirementEventColumn::TenantId.eq(target.tenant_id))
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );
        tenant_memberships::add_member(
            &ctx.db,
            target.tenant_id,
            target.owner_id,
            MembershipRole::Owner,
        )
        .await
        .unwrap();
        retirement::disable_as(&ctx.db, &actor, target.tenant_id)
            .await
            .unwrap();
        assert!(
            tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
                .await
                .unwrap()
                .disabled_at
                .is_some()
        );
        let after = retirement::inventory(&ctx.db, target.tenant_id)
            .await
            .unwrap();
        assert_eq!(after.api_keys, 0);
        assert_eq!(after.memberships, before.memberships);
        assert_eq!(after.workspace_ids, before.workspace_ids);
        assert_eq!(
            yorishiro::models::query_embedding_requests::Entity::take(
                &ctx.db,
                target.workspace_id,
                pending,
            )
            .await
            .unwrap(),
            Some(
                yorishiro::models::query_embedding_requests::QueryOutcome::Failed(
                    "tenant disabled".into()
                )
            )
        );
        let evidence = tenant_retirement_events::Entity::find()
            .filter(RetirementEventColumn::TenantId.eq(target.tenant_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].actor_user_id, Some(target.owner_id));
        assert_eq!(evidence[0].action, "disabled");
        assert!(
            retirement::disable_as(&ctx.db, &actor, target.tenant_id)
                .await
                .is_err()
        );
        assert_eq!(
            tenant_retirement_events::Entity::find()
                .filter(RetirementEventColumn::TenantId.eq(target.tenant_id))
                .count(&ctx.db)
                .await
                .unwrap(),
            1
        );
        for key in [&target.api_key, &service.plaintext] {
            request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await
                .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        }
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
    })
    .await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn retirement_cancellation_is_targeted_idempotent_and_terminal() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, TransactionTrait};
    use yorishiro::models::{query_embedding_requests as queries, queue_job_lifecycles as jobs};
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let mut ids = Vec::new();
        for workspace_id in [target.workspace_id, other.workspace_id] {
            let id = uuid::Uuid::now_v7();
            jobs::Entity::record_enqueue(
                &ctx.db,
                jobs::Enqueue {
                    id,
                    job_name: "test",
                    worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
                    workspace_id: Some(workspace_id),
                    plan: None,
                    concurrency_key: None,
                    concurrency_limit: None,
                },
            )
            .await
            .unwrap();
            ids.push(id);
        }
        let query = queries::Entity::open(
            &ctx.db,
            target.workspace_id,
            "pending",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        let txn = ctx.db.begin().await.unwrap();
        assert!(
            retirement::cancel_work(&txn, target.tenant_id)
                .await
                .is_err()
        );
        txn.rollback().await.unwrap();
        let txn = ctx.db.begin().await.unwrap();
        yorishiro::models::tenant_tenants::ActiveModel {
            id: Set(target.tenant_id),
            disabled_at: Set(Some(chrono::Utc::now().fixed_offset())),
            ..Default::default()
        }
        .update(&txn)
        .await
        .unwrap();
        retirement::cancel_work(&txn, target.tenant_id)
            .await
            .unwrap();
        retirement::cancel_work(&txn, target.tenant_id)
            .await
            .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(
            jobs::Entity::find_by_id(ids[0])
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .status,
            "cancelled"
        );
        assert_eq!(
            jobs::Entity::find_by_id(ids[1])
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        assert!(
            jobs::Entity::finish(
                &ctx.db,
                ids[0],
                Some(1),
                jobs::LifecycleStatus::Completed,
                None
            )
            .await
            .is_err()
        );
        assert!(matches!(
            jobs::Entity::start_at(&ctx.db, ids[0], chrono::Utc::now().fixed_offset())
                .await
                .unwrap(),
            jobs::Admission::Terminal
        ));
        assert_eq!(
            queries::Entity::take(&ctx.db, target.workspace_id, query)
                .await
                .unwrap(),
            Some(queries::QueryOutcome::Failed("tenant disabled".into()))
        );
    })
    .await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn disabled_tenant_rejects_credentials_issuance_and_discovery() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    use yorishiro::models::{api_keys, tenancy, tenant_tenants};

    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let service = api_keys::Entity::create_api_key(
            &ctx.db,
            target.workspace_id,
            api_keys::ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        // Set persisted state directly until a fully fenced disable operation is available.
        tenant_tenants::ActiveModel {
            id: Set(target.tenant_id),
            disabled_at: Set(Some(chrono::Utc::now().fixed_offset())),
            ..Default::default()
        }
        .update(&ctx.db)
        .await
        .unwrap();
        for key in [&target.api_key, &service.plaintext] {
            request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await
                .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        }
        for user_id in [None, Some(target.owner_id)] {
            assert!(matches!(
                api_keys::Entity::create_api_key(
                    &ctx.db,
                    target.workspace_id,
                    api_keys::ApiKeyScope::Read,
                    user_id,
                    false,
                )
                .await,
                Err(yorishiro::YorishiroError::Unauthenticated)
            ));
        }
        assert!(
            tenancy::list_workspaces_for_user(&ctx.db, target.owner_id)
                .await
                .unwrap()
                .is_empty()
        );
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
        assert!(
            retirement::inventory(&ctx.db, target.tenant_id)
                .await
                .is_ok()
        );
    })
    .await;
}

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
async fn retirement_inventory_preserves_cross_tenant_fork_integrity() {
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
        // Exercise the database guard independently of the API's tenant check.
        let error = workspace_schema_forks::ActiveModel {
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
        .unwrap_err();
        assert!(
            error.to_string().contains("source workspace mismatch"),
            "{error}"
        );
        let inventory = retirement::inventory(&ctx.db, source.tenant_id)
            .await
            .unwrap();
        assert_eq!(inventory.external_workspace_forks, 0);
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
