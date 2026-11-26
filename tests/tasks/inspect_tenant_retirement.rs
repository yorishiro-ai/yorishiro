use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use loco_rs::task::Vars;
use yorishiro::App;
use yorishiro::models::tenant_tenants::retirement;

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn retirement_audit_failure_rolls_back_disablement_and_allows_retry() {
    use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_retirement_events::Column as EventColumn;
    use yorishiro::models::{
        access_keys, query_embedding_requests as queries, queue_job_lifecycles as jobs,
        tenant_retirement_events, tenant_tenants,
    };

    if !crate::require_postgres_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let (key, _) = yorishiro::models::tenancy::issue_login_key(
            &ctx.db, target.owner_id, None, Some(target.tenant_id),
        ).await.unwrap();
        let actor = access_keys::AuthContext {
            access_key_id: key.id, workspace_id: None, tenant_id: target.tenant_id,
            user_id: Some(target.owner_id),
        };
        let job_id = uuid::Uuid::now_v7();
        jobs::Entity::record_enqueue(&ctx.db, jobs::Enqueue {
            id: job_id, job_name: "retirement-rollback", worker_class: yorishiro::workers::embedding_sync::WorkerClass::Shared,
            workspace_id: Some(target.workspace_id), plan: None, concurrency_key: None, concurrency_limit: None,
        }).await.unwrap();
        let query_id = queries::Entity::open(&ctx.db, target.workspace_id, "retirement rollback", 60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1)).await.unwrap();
        let before = retirement::inventory(&ctx.db, target.tenant_id).await.unwrap();
        // Direct write (c): isolated DDL makes the final audit write fail after the earlier mutations execute.
        ctx.db.execute_unprepared("ALTER TABLE tenant_retirement_events ADD CONSTRAINT reject_retirement_audit CHECK (false)")
            .await.unwrap();
        let error = retirement::disable_as(&ctx.db, &actor, target.tenant_id).await.unwrap_err();
        assert!(error.to_string().contains("reject_retirement_audit"), "{error}");
        assert!(tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id).await.unwrap().disabled_at.is_none());
        assert_eq!(retirement::inventory(&ctx.db, target.tenant_id).await.unwrap().access_keys, before.access_keys);
        assert_eq!(jobs::Entity::find_by_id(job_id).one(&ctx.db).await.unwrap().unwrap().status, "queued");
        assert_eq!(queries::Entity::find_by_id(query_id).one(&ctx.db).await.unwrap().unwrap().status, "pending");
        assert_eq!(tenant_retirement_events::Entity::find().filter(EventColumn::TenantId.eq(target.tenant_id))
            .count(&ctx.db).await.unwrap(), 0);
        request.get("/api/caller-identity").add_header("Authorization", format!("Bearer {}", key.plaintext))
            .await.assert_status(axum::http::StatusCode::OK);
        ctx.db.execute_unprepared("ALTER TABLE tenant_retirement_events DROP CONSTRAINT reject_retirement_audit")
            .await.unwrap();
        retirement::disable_as(&ctx.db, &actor, target.tenant_id).await.unwrap();
        assert_eq!(jobs::Entity::find_by_id(job_id).one(&ctx.db).await.unwrap().unwrap().status, "cancelled");
        assert_eq!(queries::Entity::find_by_id(query_id).one(&ctx.db).await.unwrap().unwrap().status, "failed");
        assert_eq!(retirement::inventory(&ctx.db, target.tenant_id).await.unwrap().access_keys, 0);
        assert_eq!(tenant_retirement_events::Entity::find().filter(EventColumn::TenantId.eq(target.tenant_id))
            .count(&ctx.db).await.unwrap(), 1);
    }).await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn retirement_cancellation_is_targeted_idempotent_and_terminal() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, TransactionTrait};
    use yorishiro::models::{query_embedding_requests as queries, queue_job_lifecycles as jobs};
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
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
        // Direct write (c): the disabled flag without the work cleanup a real disable performs, so `cancel_work` itself is what runs.
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
        for (id, expected) in [(ids[0], "cancelled"), (ids[1], "queued")] {
            assert_eq!(
                jobs::Entity::find_by_id(id)
                    .one(&ctx.db)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                expected
            );
        }
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
    use yorishiro::models::{access_keys, tenancy};

    if !crate::require_postgres_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let service = access_keys::Entity::create_access_key(&ctx.db, target.workspace_id, None)
            .await
            .unwrap();
        // Set persisted state directly until a fully fenced disable operation is available.
        crate::requests::fixtures::disable_tenant(&ctx, target.tenant_id).await;
        for key in [&target.api_key, &service.plaintext] {
            request
                .get("/api/caller-identity")
                .add_header("Authorization", format!("Bearer {key}"))
                .await
                .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        }
        for user_id in [None, Some(target.owner_id)] {
            assert!(matches!(
                access_keys::Entity::create_access_key(&ctx.db, target.workspace_id, user_id,)
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
            .get("/api/caller-identity")
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
async fn retirement_inventory_does_not_expose_accounts() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
        let first = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let second = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert!(
            yorishiro::models::tenant_memberships::add_member(
                &ctx.db,
                second.tenant_id,
                first.owner_id
            )
            .await
            .is_err()
        );
        for tenant in [&first, &second] {
            let inventory = retirement::inventory(&ctx.db, tenant.tenant_id)
                .await
                .unwrap();
            assert_eq!(inventory.shared_user_accounts, 0);
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
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    use yorishiro::models::{schema_schemas, workspace_schema_forks};

    if !crate::require_postgres_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
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
        // Direct write (b): exercises the database guard independently of the API's tenant check.
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
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", source.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
    })
    .await;
}

#[tokio::test]
async fn retirement_inventory_is_read_only_and_target_specific() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::_entities::{access_keys, workspace_workspaces};
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
        let keys_before = access_keys::Entity::find()
            .filter(access_keys::Column::WorkspaceId.eq(tenant.workspace_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(before.tenant_id, tenant.tenant_id);
        assert!(before.workspace_ids.contains(&tenant.workspace_id));
        assert!(before.memberships > 0);
        assert!(before.access_keys > 0);
        assert_eq!(
            before.identity_policy,
            "retain_root_accounts_delete_tenant_users"
        );
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
        let keys_after = access_keys::Entity::find()
            .filter(access_keys::Column::WorkspaceId.eq(tenant.workspace_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(keys_after).unwrap(),
            serde_json::to_value(keys_before).unwrap()
        );
        assert_eq!(
            request
                .get("/api/caller-identity")
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
