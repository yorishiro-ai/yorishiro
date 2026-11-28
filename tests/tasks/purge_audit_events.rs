use loco_rs::task::{Task, Vars};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use yorishiro::models::api_events;

async fn insert(db: &sea_orm::DatabaseConnection, age_days: i64) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    // Direct write (a): the trail only appends through requests, and a row of a chosen age cannot be made there.
    api_events::ActiveModel {
        id: Set(id),
        request_id: Set("purge-test".into()),
        phase: Set("completed".into()),
        transport: Set("rest".into()),
        event_name: Set("purge-test".into()),
        read_only: Set(true),
        user_agent: Set(String::new()),
        identity_type: Set("anonymous".into()),
        correlation_id: Set("purge-test".into()),
        resources: Set(serde_json::json!([])),
        request_parameters: Set(serde_json::json!({})),
        permissions: Set(serde_json::json!([])),
        created_at: Set((chrono::Utc::now() - chrono::Duration::days(age_days)).fixed_offset()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn the_task_deletes_only_rows_older_than_the_retention() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let old = insert(&ctx.db, 366).await;
        let recent = insert(&ctx.db, 364).await;
        yorishiro::tasks::purge_audit_events::PurgeAuditEvents
            .run(&ctx, &Vars::default())
            .await
            .unwrap();
        assert!(
            api_events::Entity::find_by_id(old)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            api_events::Entity::find_by_id(recent)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
    })
    .await;
}

#[tokio::test]
async fn a_registered_task_run_leaves_one_row_in_the_trail() {
    use loco_rs::app::Hooks;
    use loco_rs::task::Tasks;
    use sea_orm::{ColumnTrait, QueryFilter};
    crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
        let mut tasks = Tasks::default();
        yorishiro::App::register_tasks(&mut tasks);
        tasks
            .run(&ctx, "purge_audit_events", &Vars::default())
            .await
            .unwrap();
        tasks
            .run(&ctx, "maintenance_status", &Vars::default())
            .await
            .unwrap();
        let rows = api_events::Entity::find()
            .filter(yorishiro::models::_entities::api_events::Column::Transport.eq("task"))
            .all(&ctx.db)
            .await
            .unwrap();
        let find = |name: &str| rows.iter().find(|row| row.event_name == name).unwrap();
        let purge = find("purge_audit_events");
        assert_eq!(purge.identity_type, "service");
        assert_eq!(
            purge.caller.as_deref(),
            Some("yrn:yorishiro:service/purge_audit_events")
        );
        assert_eq!(purge.outcome.as_deref(), Some("success"));
        let status = find("maintenance_status");
        assert_eq!(status.identity_type, "operator");
        assert!(status.read_only);
    })
    .await;
}

#[tokio::test]
async fn the_operator_reads_rows_no_tenant_owns() {
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        request
            .get("/api/workspaces")
            .add_header("x-request-id", "no-tenant")
            .await
            .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        let rows = yorishiro::tasks::read_audit_events::rows(&ctx.db, "none", 10)
            .await
            .unwrap();
        assert!(
            rows.iter()
                .any(|row| row.request_id == "no-tenant" && row.caller.is_none())
        );
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let owned =
            yorishiro::tasks::read_audit_events::rows(&ctx.db, &tenant.tenant_id.to_string(), 50)
                .await
                .unwrap();
        assert!(
            owned
                .iter()
                .all(|row| row.tenant_id == Some(tenant.tenant_id))
        );
        assert!(
            yorishiro::tasks::read_audit_events::rows(&ctx.db, "x", 1)
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn the_task_removes_import_bodies_whose_job_never_completed() {
    use yorishiro::models::import_staging;
    crate::requests::boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let mut ids = Vec::new();
        for age_days in [8, 6] {
            // Direct write (a): a body of a chosen age cannot be staged through a request.
            let row = import_staging::ActiveModel {
                tenant_id: Set(tenant.tenant_id),
                workspace_id: Set(tenant.workspace_id),
                body: Set("body".into()),
                created_at: Set(
                    (chrono::Utc::now() - chrono::Duration::days(age_days)).fixed_offset()
                ),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
            ids.push(row.id);
        }
        yorishiro::tasks::purge_audit_events::PurgeAuditEvents
            .run(&ctx, &Vars::default())
            .await
            .unwrap();
        assert!(
            import_staging::Entity::find_by_id(ids[0])
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            import_staging::Entity::find_by_id(ids[1])
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
    })
    .await;
}
