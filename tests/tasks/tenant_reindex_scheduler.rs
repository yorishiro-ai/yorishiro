use loco_rs::{app::Hooks, task::Tasks};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, QuerySelect, QueryTrait, TransactionTrait,
};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

#[test]
fn tenant_reindex_scheduler_is_registered_in_community() {
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
async fn community_task_scans_mismatch_without_constructing_a_provider() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let settings = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap()
            .expect("the test config provides deployment metadata");
        let mismatched = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: "http://unused.invalid".into(),
            api_key: "secret".into(),
            model: "previous-model".into(),
            dimensions: metadata.dimensions,
            send_dimensions_param: false,
        });
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            &mismatched.model_name(),
            mismatched.dimensions(),
        )
        .await;
        let task = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler;
        assert!(
            ctx.shared_store
                .get::<std::sync::Arc<dyn EmbeddingProvider>>()
                .is_none(),
            "scheduler scan does not require a provider"
        );
        task.run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        let lifecycle_count = yorishiro::models::queue_job_lifecycles::Entity::find()
            .filter(
                yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .filter(yorishiro::models::queue_job_lifecycles::Column::JobName.eq("startup_reindex"))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            lifecycle_count, 1,
            "the metadata-only scan enqueued the mismatch without installing a provider"
        );
        let outbox_count = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(
                yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .filter(
                yorishiro::models::_entities::queue_job_dispatch_outbox::Column::Id.in_subquery(
                    yorishiro::models::queue_job_lifecycles::Entity::find()
                        .select_only()
                        .column(yorishiro::models::queue_job_lifecycles::Column::Id)
                        .filter(
                            yorishiro::models::queue_job_lifecycles::Column::JobName
                                .eq("startup_reindex"),
                        )
                        .filter(
                            yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                                .eq(fixture.workspace_id),
                        )
                        .into_query(),
                ),
            )
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(outbox_count, 1);
        task.run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .unwrap();
        let repeated_lifecycle_count = yorishiro::models::queue_job_lifecycles::Entity::find()
            .filter(
                yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                    .eq(fixture.workspace_id),
            )
            .filter(yorishiro::models::queue_job_lifecycles::Column::JobName.eq("startup_reindex"))
            .count(&ctx.db)
            .await
            .unwrap();
        let repeated_outbox_count =
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
                                    yorishiro::models::queue_job_lifecycles::Column::JobName
                                        .eq("startup_reindex"),
                                )
                                .filter(
                                    yorishiro::models::queue_job_lifecycles::Column::WorkspaceId
                                        .eq(fixture.workspace_id),
                                )
                                .into_query(),
                        ),
                )
                .count(&ctx.db)
                .await
                .unwrap();
        assert_eq!(repeated_lifecycle_count, 1);
        assert_eq!(repeated_outbox_count, 1);
        assert!(
            yorishiro::models::queue_job_lifecycles::Entity::has_active(
                &ctx.db,
                "startup_reindex",
                fixture.workspace_id
            )
            .await
            .unwrap()
        );
    })
    .await;
}

#[tokio::test]
async fn enqueue_rejection_is_workspace_scoped_and_scan_continues() {
    use sea_orm::DbBackend;

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let first = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs {
                workspace_name: "a-failing-workspace".into(),
                ..Default::default()
            },
        )
        .await;
        let transaction = ctx.db.begin().await.unwrap();
        let second_workspace = yorishiro::models::tenancy::create_workspace(
            &transaction,
            first.tenant_id,
            "z-successful-workspace",
            None,
            None,
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        let settings = ctx.config.settings::<yorishiro::data::settings::Settings>().unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap()
            .unwrap();
        for workspace_id in [first.workspace_id, second_workspace.id] {
            stamp_workspace(&ctx, workspace_id, "previous-model", metadata.dimensions).await;
        }
        let failed = first.workspace_id;
        let succeeds = second_workspace.id;
        let sql = match ctx.db.get_database_backend() {
            DbBackend::Postgres => format!(
                "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT reject_one_workspace CHECK (workspace_id <> '{failed}')"
            ),
            DbBackend::Sqlite => format!(
                "CREATE TRIGGER reject_one_workspace BEFORE INSERT ON queue_job_dispatch_outbox WHEN hex(NEW.workspace_id) = '{}' BEGIN SELECT RAISE(ABORT, 'reject selected workspace'); END",
                failed.as_bytes().iter().map(|byte| format!("{byte:02X}")).collect::<String>()
            ),
            backend => panic!("unsupported test backend {backend:?}"),
        };
        ctx.db.execute_unprepared(&sql).await.unwrap();

        let error = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await.expect_err("targeted outbox rejection must be returned");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&failed.to_string()), "{diagnostic}");
        assert!(yorishiro::models::queue_job_lifecycles::Entity::has_active(
            &ctx.db, "startup_reindex", succeeds
        ).await.unwrap());
        assert!(!yorishiro::models::queue_job_lifecycles::Entity::has_active(
            &ctx.db, "startup_reindex", failed
        ).await.unwrap());
        let good_outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId.eq(succeeds))
            .count(&ctx.db).await.unwrap();
        let bad_outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find()
            .filter(yorishiro::models::_entities::queue_job_dispatch_outbox::Column::WorkspaceId.eq(failed))
            .count(&ctx.db).await.unwrap();
        assert_eq!(good_outbox, 1);
        assert_eq!(bad_outbox, 0);
    }).await;
}

#[tokio::test]
async fn all_enqueue_failures_are_aggregated_with_each_workspace_id() {
    use sea_orm::DbBackend;

    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let first = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let transaction = ctx.db.begin().await.unwrap();
        let second_workspace = yorishiro::models::tenancy::create_workspace(
            &transaction,
            first.tenant_id,
            "second-workspace",
            None,
            None,
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        let settings = ctx.config.settings::<yorishiro::data::settings::Settings>().unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap().unwrap();
        for workspace_id in [first.workspace_id, second_workspace.id] {
            stamp_workspace(&ctx, workspace_id, "previous-model", metadata.dimensions).await;
        }
        let fault = match ctx.db.get_database_backend() {
            DbBackend::Postgres => "ALTER TABLE queue_job_dispatch_outbox ADD CONSTRAINT reject_all_dispatches CHECK (false)",
            DbBackend::Sqlite => "CREATE TRIGGER reject_all_dispatches BEFORE INSERT ON queue_job_dispatch_outbox BEGIN SELECT RAISE(ABORT, 'reject all dispatches'); END",
            backend => panic!("unsupported test backend {backend:?}"),
        };
        ctx.db.execute_unprepared(fault).await.unwrap();
        let error = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await.expect_err("both workspace enqueue attempts must fail");
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&first.workspace_id.to_string()), "{diagnostic}");
        assert!(diagnostic.contains(&second_workspace.id.to_string()), "{diagnostic}");
        for workspace_id in [first.workspace_id, second_workspace.id] {
            assert!(!yorishiro::models::queue_job_lifecycles::Entity::has_active(
                &ctx.db, "startup_reindex", workspace_id,
            ).await.unwrap());
        }
    }).await;
}

#[tokio::test]
async fn missing_lifecycle_table_returns_workspace_lookup_error() {
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(|request, ctx| async move {
        let mut ctx = ctx;
        configure_deployment_metadata(&mut ctx);
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let settings = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap();
        let metadata = yorishiro::services::embedding::deployment_embedding_metadata(&settings)
            .unwrap()
            .expect("the test config provides deployment metadata");
        stamp_workspace(
            &ctx,
            fixture.workspace_id,
            "previous-model",
            metadata.dimensions,
        )
        .await;
        ctx.db
            .execute_unprepared(
                "ALTER TABLE queue_job_lifecycles RENAME TO queue_job_lifecycles_hidden",
            )
            .await
            .unwrap();
        let error = yorishiro::tasks::tenant_reindex_scheduler::TenantReindexScheduler
            .run_at(&ctx, &loco_rs::task::Vars::default(), chrono::Utc::now())
            .await
            .expect_err("missing lifecycle table must fail lookup");
        assert!(
            error
                .to_string()
                .contains(&fixture.workspace_id.to_string()),
            "{error}"
        );
        ctx.db
            .execute_unprepared(
                "ALTER TABLE queue_job_lifecycles_hidden RENAME TO queue_job_lifecycles",
            )
            .await
            .unwrap();
    })
    .await;
}

async fn stamp_workspace(
    ctx: &loco_rs::app::AppContext,
    workspace_id: uuid::Uuid,
    model: &str,
    dimensions: usize,
) {
    use sea_orm::ActiveValue::Set;
    let workspace = yorishiro::models::workspace_workspaces::Entity::find_by_id(workspace_id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let mut active = workspace.into_active_model();
    active.embedding_model = Set(Some(model.to_owned()));
    active.embedding_dimensions = Set(Some(i32::try_from(dimensions).unwrap()));
    active.update(&ctx.db).await.unwrap();
}

fn configure_deployment_metadata(ctx: &mut loco_rs::app::AppContext) {
    #[cfg(feature = "enterprise")]
    {
        use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
        ctx.shared_store
            .insert(std::sync::Arc::new(LicenceState::default()));
        assert!(
            !ctx.shared_store
                .get::<std::sync::Arc<LicenceState>>()
                .unwrap()
                .is_active()
        );
    }
    let mut value = ctx.config.settings.as_ref().unwrap().clone();
    value["embedding"]["provider"] = serde_json::json!("openai");
    value["embedding"]["base_url"] = serde_json::json!("http://unused.invalid");
    value["embedding"]["model"] = serde_json::json!("test-deployment-model");
    ctx.config.settings = Some(value);
}
