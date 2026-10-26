use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use yorishiro::App;
use yorishiro::models::entity_entities::{self, CreateEntityInput, ListEntitiesQuery};
use yorishiro::models::schema_schemas;

async fn seed_schema(request: &loco_rs::TestServer, ctx: &loco_rs::app::AppContext) -> uuid::Uuid {
    let tenant = TestTenant::create(request, ctx, TenantArgs::default()).await;
    let definition = serde_json::json!({
        "name": "notes",
        "entity_types": {
            "note": { "fields": {}, "required": [] }
        }
    });
    let def = serde_json::from_value(definition).expect("parse definition");
    schema_schemas::create_schema(
        &ctx.db,
        tenant.tenant_id,
        tenant.workspace_id,
        def,
        None,
        None,
    )
    .await
    .expect("create schema");

    tenant.workspace_id
}

#[tokio::test]
async fn content_entities_crud_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = seed_schema(&request, &ctx).await;
        let db = ctx.db.clone();

        let input = CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: serde_json::json!({"title": "first"}),
        };
        let created = entity_entities::create(&db, workspace_id, input, None)
            .await
            .expect("create");
        assert_eq!(created.data["title"], "first");

        let fetched = entity_entities::get(&db, workspace_id, created.id)
            .await
            .expect("get");
        assert_eq!(fetched.id, created.id);

        let batch = entity_entities::get_batch(&db, workspace_id, &[created.id])
            .await
            .expect("get_batch");
        assert_eq!(batch.len(), 1);
        assert!(batch.contains_key(&created.id));

        let listed = entity_entities::list(&db, workspace_id, ListEntitiesQuery::default())
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);

        let exported = entity_entities::export_all(&db, workspace_id)
            .await
            .expect("export_all");
        assert_eq!(exported.len(), 1);

        let counted = entity_entities::count(&db, workspace_id)
            .await
            .expect("count");
        assert_eq!(counted, 1);

        let updated = entity_entities::update(
            &db,
            workspace_id,
            entity_entities::UpdateEntityInput {
                id: created.id,
                data: serde_json::json!({"title": "second"}),
                updated_by: None,
            },
        )
        .await
        .expect("update");
        assert_eq!(updated.data["title"], "second");
        assert!(
            updated.updated_at > created.updated_at,
            "updated_at should advance on update"
        );

        entity_entities::delete(&db, workspace_id, created.id)
            .await
            .expect("delete");

        let after_delete = entity_entities::count(&db, workspace_id)
            .await
            .expect("count after delete");
        assert_eq!(after_delete, 0);
    })
    .await;
}

#[tokio::test]
async fn undo_job_restores_and_counts_a_missing_entity_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = seed_schema(&request, &ctx).await;
        let db = ctx.db.clone();

        let input = CreateEntityInput {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: serde_json::json!({"title": "original"}),
        };
        let created = entity_entities::create(&db, workspace_id, input, None)
            .await
            .expect("create");

        let job_id = uuid::Uuid::now_v7();
        entity_entities::snapshot(&db, workspace_id, created.id, job_id)
            .await
            .expect("snapshot existing entity");
        entity_entities::update(
            &db,
            workspace_id,
            entity_entities::UpdateEntityInput {
                id: created.id,
                data: serde_json::json!({"title": "changed"}),
                updated_by: None,
            },
        )
        .await
        .expect("change existing entity");
        let deleted = entity_entities::create(
            &db,
            workspace_id,
            CreateEntityInput {
                schema_name: "notes".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title": "gone"}),
            },
            None,
        )
        .await
        .expect("create entity to delete");
        entity_entities::snapshot(&db, workspace_id, deleted.id, job_id)
            .await
            .expect("snapshot entity to delete");
        entity_entities::delete(&db, workspace_id, deleted.id)
            .await
            .expect("delete snapshotted entity");

        let report = entity_entities::undo_job(&db, workspace_id, job_id)
            .await
            .expect("undo_job");
        assert_eq!(report.restored, 1);
        assert_eq!(report.missing, 1);

        let restored = entity_entities::get(&db, workspace_id, created.id)
            .await
            .expect("get after undo");
        assert_eq!(restored.data["title"], "original");
    })
    .await;
}

/// A JSONB containment filter on `list()` must return `BackendUnsupported` on SQLite,
/// because the column is `TEXT` there and the JSONB `@>` operator is PostgreSQL-only.
#[tokio::test]
async fn entity_list_filter_returns_backend_unsupported_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = seed_schema(&request, &ctx).await;
        let db = ctx.db.clone();

        entity_entities::create(
            &db,
            workspace_id,
            CreateEntityInput {
                schema_name: "notes".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"status": "active"}),
            },
            None,
        )
        .await
        .expect("create entity");

        let result = entity_entities::list(
            &db,
            workspace_id,
            ListEntitiesQuery {
                filter: Some(serde_json::json!({"status": "active"})),
                ..Default::default()
            },
        )
        .await;

        assert!(
            matches!(
                result,
                Err(yorishiro::error::YorishiroError::BackendUnsupported { .. })
            ),
            "JSONB filter on SQLite must return BackendUnsupported, got: {result:?}"
        );
    })
    .await;
}

/// The database holds one entity, so every FTS row belongs to it.
async fn fts_rows(db: &sea_orm::DatabaseConnection) -> i64 {
    use sea_orm::FromQueryResult;
    use sea_orm::sea_query::{Alias, Asterisk, Expr, Func, Query};
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let query = Query::select()
        .expr_as(Func::count(Expr::col(Asterisk)), Alias::new("n"))
        .from(Alias::new("entity_fts"))
        .to_owned();
    Count::find_by_statement(db.get_database_backend().build(&query))
        .one(db)
        .await
        .expect("count FTS rows")
        .expect("count row")
        .n
}

/// The token triggers update the row they just wrote.
/// That nested update must not reach `entity_fts_au`, or every entity would be indexed twice.
#[tokio::test]
async fn token_rotation_does_not_duplicate_fts_rows_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let workspace_id = seed_schema(&request, &ctx).await;
        let db = ctx.db.clone();
        let created = entity_entities::create(
            &db,
            workspace_id,
            CreateEntityInput {
                schema_name: "notes".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title": "first"}),
            },
            None,
        )
        .await
        .expect("create");
        assert_eq!(fts_rows(&db).await, 1, "one row after insert");

        entity_entities::update(
            &db,
            workspace_id,
            entity_entities::UpdateEntityInput {
                id: created.id,
                data: serde_json::json!({"title": "second"}),
                updated_by: None,
            },
        )
        .await
        .expect("update");
        assert_eq!(fts_rows(&db).await, 1, "one row after a data update");

        // Exercise a non-data update independently of the model's data-update operation.
        entity_entities::Entity::update_many()
            .col_expr(
                yorishiro::models::_entities::entity_entities::Column::UpdatedBy,
                sea_orm::sea_query::Expr::value(Option::<uuid::Uuid>::None),
            )
            .filter(yorishiro::models::_entities::entity_entities::Column::Id.eq(created.id))
            .exec(&db)
            .await
            .expect("update a column other than data");
        assert_eq!(fts_rows(&db).await, 1, "one row after a non-data update");
    })
    .await;
}
