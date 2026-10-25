use loco_rs::{app::Hooks, task::Tasks};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    QuerySelect, QueryTrait, TransactionTrait,
};
use std::sync::Arc;
use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

#[test]
fn tenant_reindex_scheduler_is_registered_in_enterprise() {
    let mut tasks = Tasks::default();
    yorishiro::App::register_tasks(&mut tasks);
    assert!(
        tasks
            .names()
            .iter()
            .any(|name| name == "tenant_reindex_scheduler")
    );
}

#[tokio::test]
async fn licensed_task_ticks_but_unlicensed_task_scans_without_provider() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        use yorishiro::edition::ee::models::tenant_reindex_schedules::{get, set};
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        set(&ctx.db, fixture.tenant_id, "PT6H", None).await.unwrap();
        let before = get(&ctx.db, fixture.tenant_id)
            .await
            .unwrap()
            .unwrap()
            .scheduled_for
            .unwrap();

        let licensed = Arc::new(LicenceState::licensed(LicenceClaims {
            sub: "test".into(),
            plan: "pro".into(),
            exp: chrono::Utc::now().timestamp() + 3600,
        }));
        ctx.shared_store.insert(licensed);
        let due = before.to_utc();
        yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), due)
            .await
            .unwrap();
        let next = get(&ctx.db, fixture.tenant_id)
            .await
            .unwrap()
            .unwrap()
            .scheduled_for
            .unwrap();
        assert_eq!(next.to_utc(), due + chrono::Duration::hours(6));

        ctx.shared_store.insert(Arc::new(LicenceState::default()));
        let task = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler;
        let lifecycle_count = || {
            yorishiro::models::queue_job_lifecycles::Entity::find()
                .filter(
                    yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                        .eq(fixture.workspace_id),
                )
                .filter(
                    yorishiro::models::queue_job_lifecycles::Column::JobName.eq("startup_reindex"),
                )
        };
        let outbox_count = || {
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
                .filter(
                    yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                        .eq(fixture.workspace_id),
                )
                .filter(
                    yorishiro::models::_entities::queue_job_dispatch_outbox::Column::Id
                        .in_subquery(
                            yorishiro::models::queue_job_lifecycles::Entity::find()
                                .select_only()
                                .column(yorishiro::models::queue_job_lifecycles::Column::Id)
                                .filter(
                                    yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                                        .eq(fixture.workspace_id),
                                )
                                .filter(
                                    yorishiro::models::queue_job_lifecycles::Column::JobName
                                        .eq("startup_reindex"),
                                )
                                .into_query(),
                        ),
                )
        };
        assert_eq!(lifecycle_count().count(&ctx.db).await.unwrap(), 0);
        assert_eq!(outbox_count().count(&ctx.db).await.unwrap(), 0);
        let old = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "secret".into(),
            model: "previous-model".into(),
            dimensions: 768,
            send_dimensions_param: false,
        });
        yorishiro::models::entity_embeddings::reindex_workspace(
            &ctx.db,
            fixture.workspace_id,
            &[],
            &old,
        )
        .await
        .unwrap();
        assert!(
            ctx.shared_store
                .get::<Arc<dyn EmbeddingProvider>>()
                .is_none(),
            "scheduler scan does not require a provider"
        );
        task.run_at(&ctx, &loco_rs::task::Vars::default(), due)
            .await
            .unwrap();
        assert!(
            yorishiro::models::queue_job_lifecycles::Entity::has_active(
                &ctx.db,
                "startup_reindex",
                fixture.workspace_id
            )
            .await
            .unwrap()
        );
        assert_eq!(lifecycle_count().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(outbox_count().count(&ctx.db).await.unwrap(), 1);
        task.run_at(&ctx, &loco_rs::task::Vars::default(), due)
            .await
            .unwrap();
        assert_eq!(lifecycle_count().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(outbox_count().count(&ctx.db).await.unwrap(), 1);
        assert_eq!(
            get(&ctx.db, fixture.tenant_id)
                .await
                .unwrap()
                .unwrap()
                .scheduled_for
                .unwrap(),
            next
        );
    })
    .await;
}

#[tokio::test]
async fn metadata_scan_uses_workspace_provider_key_without_constructing_provider() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        ctx.shared_store.insert(Arc::new(LicenceState::default()));
        assert!(
            !ctx.shared_store
                .get::<Arc<LicenceState>>()
                .unwrap()
                .is_active()
        );
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let txn = ctx.db.begin().await.unwrap();
        yorishiro::edition::ee::models::workspace_embedding_keys::set(
            &txn,
            fixture.workspace_id,
            "http://unused.invalid",
            "workspace-model",
            "test-key",
            768,
            false,
            Some(768),
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let workspace =
            yorishiro::models::workspace_workspaces::Entity::find_by_id(fixture.workspace_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
        let mut workspace = workspace.into_active_model();
        workspace.embedding_model = sea_orm::ActiveValue::Set(Some("previous-model".into()));
        workspace.embedding_dimensions = sea_orm::ActiveValue::Set(Some(768));
        workspace.update(&ctx.db).await.unwrap();
        let task = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler;
        task.run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        assert!(
            yorishiro::models::queue_job_lifecycles::Entity::has_active(
                &ctx.db,
                "startup_reindex",
                fixture.workspace_id,
            )
            .await
            .unwrap()
        );
        let before = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(
                yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .count(&ctx.db)
            .await
            .unwrap();
        task.run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        assert_eq!(
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
                .filter(
                    yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                        .eq(fixture.workspace_id)
                )
                .count(&ctx.db)
                .await
                .unwrap(),
            before
        );
    })
    .await;
}

fn configure_deployment_metadata(ctx: &mut loco_rs::app::AppContext) {
    let mut value = ctx.config.settings.as_ref().unwrap().clone();
    value["embedding"]["provider"] = serde_json::json!("openai");
    value["embedding"]["base_url"] = serde_json::json!("http://unused.invalid");
    value["embedding"]["model"] = serde_json::json!("current-model");
    ctx.config.settings = Some(value);
}
