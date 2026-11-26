use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use yorishiro::App;
use yorishiro::models::_entities::entity_entities::Column;
use yorishiro::models::entity_entities::{self, CreateEntityInput, ListEntitiesQuery};
use yorishiro::models::schema_schemas;

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
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
    if !crate::require_sqlite_backend() {
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

        let batch = entity_entities::Entity::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .filter(Column::Id.is_in([created.id]))
            .all(&db)
            .await
            .expect("find fixture batch");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].id, created.id);

        let listed = entity_entities::list(&db, workspace_id, ListEntitiesQuery::default())
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);

        let persisted = entity_entities::Entity::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .all(&db)
            .await
            .expect("inspect fixture rows");
        assert_eq!(persisted.len(), 1);

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
                expected_updated_at: None,
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
