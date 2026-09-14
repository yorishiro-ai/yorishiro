use super::boot_request;
use super::fixtures::{self, TenantArgs};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    TransactionTrait,
};
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::{
    entity_entities, schema_schemas, tenant_tenants, workspace_schema_fork_heads,
    workspace_schema_forks, workspace_workspaces,
};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::auth::ApiKeyScope;

async fn create_workspace(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    name: &str,
) -> workspace_workspaces::Model {
    workspace_workspaces::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        name: ActiveValue::Set(name.into()),
        status: ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap()
}

async fn insert_reference(
    ctx: &loco_rs::app::AppContext,
    workspace_id: Uuid,
    schema_id: Uuid,
    schema_version: i32,
) -> Uuid {
    entity_entities::ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        schema_id: ActiveValue::Set(schema_id),
        schema_version: ActiveValue::Set(schema_version),
        entity_type: ActiveValue::Set("task".into()),
        data: ActiveValue::Set(serde_json::json!({"title": "reference"})),
        created_by: ActiveValue::Set(None),
        updated_by: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap()
    .id
}

#[tokio::test]
#[serial]
async fn schema_fork_crud_copy_follow_and_history_guards() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, source_workspace_id, owner_id, source_key) =
            fixtures::create_tenant_workspace_owner(
                &ctx,
                TenantArgs {
                    key_scope: ApiKeyScope::Schema,
                    ..Default::default()
                },
            )
            .await;
        let schema_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_response.status_code(), 201);
        let schema: serde_json::Value = schema_response.json();
        let source_schema_id: Uuid = schema["schema"]["id"].as_str().unwrap().parse().unwrap();

        let target = create_workspace(&ctx, tenant_id, "target").await;
        let target_key =
            fixtures::issue_api_key(&ctx, target.id, owner_id, ApiKeyScope::Schema, false).await;
        let read_key =
            fixtures::issue_api_key(&ctx, target.id, owner_id, ApiKeyScope::Read, false).await;

        let denied = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(denied.status_code(), 403);

        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let fork: serde_json::Value = created.json();
        assert_eq!(
            fork["source_schema_id"].as_str().unwrap(),
            source_schema_id.to_string()
        );
        assert_eq!(fork["customized"], false);
        assert_eq!(fork["source_schema_version"], schema["schema"]["version"]);
        assert_eq!(fork["definition"], schema["schema"]["definition"]);
        let fork_id = fork["id"].as_str().unwrap().to_owned();
        let original_head_id: Uuid = fork["fork_schema_id"].as_str().unwrap().parse().unwrap();
        let original_head_version = fork["fork_schema_version"].as_i64().unwrap() as i32;
        let history_update = workspace_schema_fork_heads::Entity::update_many()
            .col_expr(
                workspace_schema_fork_heads::Column::SchemaId,
                sea_orm::sea_query::Expr::value(original_head_id),
            )
            .filter(
                workspace_schema_fork_heads::Column::ForkId.eq(fork_id.parse::<Uuid>().unwrap()),
            )
            .exec(&ctx.db)
            .await;
        assert!(
            history_update.is_err(),
            "fork-head history rows must be immutable"
        );

        let listed = request
            .get("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(listed.status_code(), 200);
        let listed: serde_json::Value = listed.json();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], fork["id"]);

        let fetched = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(fetched.status_code(), 200);
        assert_eq!(
            fetched.json::<serde_json::Value>()["definition"],
            fork["definition"]
        );

        let duplicate = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(duplicate.status_code(), 409);

        let local = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "definition": fork["definition"],
                "expected_fork_schema_id": fork["fork_schema_id"]
            }))
            .await;
        assert_eq!(local.status_code(), 200, "{}", local.text());
        let local: serde_json::Value = local.json();
        assert_eq!(local["customized"], true);
        let local_updated_at =
            chrono::DateTime::parse_from_rfc3339(local["updated_at"].as_str().unwrap()).unwrap();
        let original_updated_at =
            chrono::DateTime::parse_from_rfc3339(fork["updated_at"].as_str().unwrap()).unwrap();
        assert!(
            local_updated_at >= original_updated_at,
            "local customization must not move updated_at backwards"
        );

        let stale = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "definition": local["definition"],
                "expected_fork_schema_id": original_head_id
            }))
            .await;
        assert_eq!(stale.status_code(), 409);

        let source_v2 = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(source_v2.status_code(), 201, "{}", source_v2.text());
        let source_v2: serde_json::Value = source_v2.json();
        let source_v2_id = source_v2["schema"]["id"].as_str().unwrap();

        let follow = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "action": "follow",
                "expected_fork_schema_id": local["fork_schema_id"],
                "expected_source_schema_id": source_v2_id
            }))
            .await;
        assert_eq!(follow.status_code(), 409);

        let followed = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "action": "follow",
                "force": true,
                "expected_fork_schema_id": local["fork_schema_id"],
                "expected_source_schema_id": source_v2_id
            }))
            .await;
        assert_eq!(followed.status_code(), 200, "{}", followed.text());
        let followed: serde_json::Value = followed.json();
        assert_eq!(followed["customized"], false);
        assert_eq!(followed["source_schema_id"], source_v2_id);
        assert_eq!(followed["definition"], source_v2["schema"]["definition"]);
        let current_head_id: Uuid = followed["fork_schema_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let current_head_version = followed["fork_schema_version"].as_i64().unwrap() as i32;

        let old_reference =
            insert_reference(&ctx, target.id, original_head_id, original_head_version).await;
        let delete_old_referenced = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .await;
        assert_eq!(delete_old_referenced.status_code(), 409);
        entity_entities::Entity::delete_by_id(old_reference)
            .exec(&ctx.db)
            .await
            .unwrap();

        let current_reference =
            insert_reference(&ctx, target.id, current_head_id, current_head_version).await;
        let delete_current_referenced = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .await;
        assert_eq!(delete_current_referenced.status_code(), 409);
        entity_entities::Entity::delete_by_id(current_reference)
            .exec(&ctx.db)
            .await
            .unwrap();

        let deleted = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .await;
        assert_eq!(deleted.status_code(), 204);

        assert!(
            workspace_schema_forks::Entity::find_by_id(
                fork["id"].as_str().unwrap().parse::<Uuid>().unwrap()
            )
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none()
        );
        assert_eq!(
            workspace_schema_fork_heads::Entity::find()
                .filter(
                    workspace_schema_fork_heads::Column::ForkId.eq(fork["id"]
                        .as_str()
                        .unwrap()
                        .parse::<Uuid>()
                        .unwrap())
                )
                .count(&ctx.db)
                .await
                .unwrap(),
            0
        );
        assert!(
            schema_schemas::Entity::find_by_id(original_head_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            schema_schemas::Entity::find_by_id(current_head_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn schema_forks_reject_transitive_workspace_cycles() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_a, owner_id, key_a) = fixtures::create_tenant_workspace_owner(
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let workspace_b = create_workspace(&ctx, tenant_id, "workspace-b").await;
        let workspace_c = create_workspace(&ctx, tenant_id, "workspace-c").await;
        let key_b =
            fixtures::issue_api_key(&ctx, workspace_b.id, owner_id, ApiKeyScope::Schema, false)
                .await;
        let key_c =
            fixtures::issue_api_key(&ctx, workspace_c.id, owner_id, ApiKeyScope::Schema, false)
                .await;

        let mut schemas = Vec::new();
        for key in [&key_a, &key_b, &key_c] {
            let response = request
                .post("/api/schemas")
                .add_header("Authorization", format!("Bearer {key}"))
                .json(&serde_json::json!({ "template_id": "task-management" }))
                .await;
            assert_eq!(response.status_code(), 201, "{}", response.text());
            let body: serde_json::Value = response.json();
            schemas.push(
                body["schema"]["id"]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap(),
            );
        }

        let b_follows_a = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {key_b}"))
            .json(&serde_json::json!({
                "source_workspace_id": workspace_a,
                "source_schema_id": schemas[0]
            }))
            .await;
        assert_eq!(b_follows_a.status_code(), 201, "{}", b_follows_a.text());

        let c_follows_b = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {key_c}"))
            .json(&serde_json::json!({
                "source_workspace_id": workspace_b.id,
                "source_schema_id": schemas[1]
            }))
            .await;
        assert_eq!(c_follows_b.status_code(), 201, "{}", c_follows_b.text());

        let cycle = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {key_a}"))
            .json(&serde_json::json!({
                "source_workspace_id": workspace_c.id,
                "source_schema_id": schemas[2]
            }))
            .await;
        assert_eq!(cycle.status_code(), 409, "{}", cycle.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn concurrent_opposite_forks_allow_one_edge_and_reject_the_other() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_a, owner_id, key_a) = fixtures::create_tenant_workspace_owner(
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let workspace_b = create_workspace(&ctx, tenant_id, "opposite-cycle-b").await;
        let key_b =
            fixtures::issue_api_key(&ctx, workspace_b.id, owner_id, ApiKeyScope::Schema, false)
                .await;

        let schema_a_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key_a}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_a_response.status_code(), 201);
        let schema_a: serde_json::Value = schema_a_response.json();
        let schema_a_id = schema_a["schema"]["id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();

        let schema_b_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key_b}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_b_response.status_code(), 201);
        let schema_b: serde_json::Value = schema_b_response.json();
        let schema_b_id = schema_b["schema"]["id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let txn_a = ctx.db.begin().await.unwrap();
        let txn_b = ctx.db.begin().await.unwrap();
        let barrier_a = barrier.clone();
        let barrier_b = barrier;
        let create_a = async {
            barrier_a.wait().await;
            let result = yorishiro::ee::models::workspace_schema_forks::create(
                &txn_a,
                tenant_id,
                workspace_a,
                yorishiro::ee::models::workspace_schema_forks::CreateInput {
                    source_workspace_id: workspace_b.id,
                    source_schema_id: schema_b_id,
                },
            )
            .await;
            if result.is_ok() {
                txn_a.commit().await.unwrap();
            } else {
                txn_a.rollback().await.unwrap();
            }
            result
        };
        let create_b = async {
            barrier_b.wait().await;
            let result = yorishiro::ee::models::workspace_schema_forks::create(
                &txn_b,
                tenant_id,
                workspace_b.id,
                yorishiro::ee::models::workspace_schema_forks::CreateInput {
                    source_workspace_id: workspace_a,
                    source_schema_id: schema_a_id,
                },
            )
            .await;
            if result.is_ok() {
                txn_b.commit().await.unwrap();
            } else {
                txn_b.rollback().await.unwrap();
            }
            result
        };
        let (result_a, result_b) = tokio::join!(create_a, create_b);
        assert_ne!(
            result_a.is_ok(),
            result_b.is_ok(),
            "exactly one opposite edge may commit"
        );
        assert!(
            matches!(result_a, Err(yorishiro::YorishiroError::Conflict { .. }))
                || matches!(result_b, Err(yorishiro::YorishiroError::Conflict { .. })),
            "the losing edge must be rejected as a cycle conflict"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn fork_creation_shares_the_schema_version_lock() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, source_workspace_id, _owner_id, source_key) =
            fixtures::create_tenant_workspace_owner(
                &ctx,
                TenantArgs {
                    key_scope: ApiKeyScope::Schema,
                    ..Default::default()
                },
            )
            .await;
        let source_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(source_response.status_code(), 201);
        let source: serde_json::Value = source_response.json();
        let source_schema_id = source["schema"]["id"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap();
        let definition = serde_json::from_value(source["schema"]["definition"].clone()).unwrap();
        let target = create_workspace(&ctx, tenant_id, "version-race-target").await;

        let first = ctx.db.begin().await.unwrap();
        yorishiro::models::schema_schemas::lock_version(&first, target.id, "task-management")
            .await
            .unwrap();

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let second_barrier = barrier.clone();
        let second_db = ctx.db.clone();
        let mut second = tokio::spawn(async move {
            let txn = second_db.begin().await.unwrap();
            second_barrier.wait().await;
            let fork = yorishiro::ee::models::workspace_schema_forks::create(
                &txn,
                tenant_id,
                target.id,
                yorishiro::ee::models::workspace_schema_forks::CreateInput {
                    source_workspace_id,
                    source_schema_id,
                },
            )
            .await
            .unwrap();
            txn.commit().await.unwrap();
            fork
        });
        barrier.wait().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(250), &mut second)
                .await
                .is_err(),
            "fork creation must wait for the canonical workspace and schema-name lock"
        );

        let ordinary = yorishiro::models::schema_schemas::create_schema(
            &first, tenant_id, target.id, definition, None, None,
        )
        .await
        .unwrap();
        assert_eq!(ordinary.0.version, 1);
        first.commit().await.unwrap();

        let fork = second.await.unwrap();
        assert_eq!(fork.fork_schema_version, 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn schema_fork_rls_and_integrity_reject_cross_tenant_rows() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_a, source_workspace, owner_a, source_key) =
            fixtures::create_tenant_workspace_owner(
                &ctx,
                TenantArgs {
                    tenant_name: "tenant-a".into(),
                    owner_email: "owner-a@example.com".into(),
                    key_scope: ApiKeyScope::Schema,
                    ..Default::default()
                },
            )
            .await;
        let source_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        let source: serde_json::Value = source_response.json();
        let source_schema_id = source["schema"]["id"].as_str().unwrap();
        let target = create_workspace(&ctx, tenant_a, "target-a").await;
        let target_key =
            fixtures::issue_api_key(&ctx, target.id, owner_a, ApiKeyScope::Schema, false).await;
        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let fork: serde_json::Value = created.json();
        let fork_id = fork["id"].as_str().unwrap().parse::<Uuid>().unwrap();

        let (tenant_b, workspace_b, _owner_b, _key_b) = fixtures::create_tenant_workspace_owner(
            &ctx,
            TenantArgs {
                tenant_name: "tenant-b".into(),
                workspace_name: "workspace-b".into(),
                owner_email: "owner-b@example.com".into(),
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let db = ctx.shared_store.get::<yorishiro::db::DbHandle>().unwrap();

        let tenant_b_txn = db
            .tenant
            .begin_for_workspace(tenant_b, workspace_b)
            .await
            .unwrap();
        assert!(
            workspace_schema_forks::Entity::find_by_id(fork_id)
                .one(&tenant_b_txn)
                .await
                .unwrap()
                .is_none()
        );
        tenant_b_txn.rollback().await.unwrap();

        let target_txn = db
            .tenant
            .begin_for_workspace(tenant_a, target.id)
            .await
            .unwrap();
        let update = workspace_schema_forks::Entity::update_many()
            .col_expr(
                workspace_schema_forks::Column::TenantId,
                sea_orm::sea_query::Expr::value(tenant_b),
            )
            .filter(workspace_schema_forks::Column::Id.eq(fork_id))
            .exec(&target_txn)
            .await;
        assert!(update.is_err(), "cross-tenant UPDATE must be rejected");
        target_txn.rollback().await.unwrap();

        let original = workspace_schema_forks::Entity::find_by_id(fork_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let insert_txn = db
            .tenant
            .begin_for_workspace(tenant_a, target.id)
            .await
            .unwrap();
        let insert = workspace_schema_forks::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_b),
            workspace_id: ActiveValue::Set(target.id),
            source_workspace_id: ActiveValue::Set(original.source_workspace_id),
            source_schema_id: ActiveValue::Set(original.source_schema_id),
            source_schema_version: ActiveValue::Set(original.source_schema_version),
            source_schema_name: ActiveValue::Set(original.source_schema_name),
            fork_schema_id: ActiveValue::Set(original.fork_schema_id),
            customized: ActiveValue::Set(false),
            ..Default::default()
        }
        .insert(&insert_txn)
        .await;
        assert!(insert.is_err(), "cross-tenant INSERT must be rejected");
        insert_txn.rollback().await.unwrap();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sqlite_schema_fork_integrity_rejects_cross_tenant_rows() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_a, source_workspace, owner_id, source_key) =
            fixtures::create_tenant_workspace_owner(
                &ctx,
                TenantArgs {
                    key_scope: ApiKeyScope::Schema,
                    ..Default::default()
                },
            )
            .await;
        let source_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        let source: serde_json::Value = source_response.json();
        let target = create_workspace(&ctx, tenant_a, "sqlite-target").await;
        let target_key =
            fixtures::issue_api_key(&ctx, target.id, owner_id, ApiKeyScope::Schema, false).await;
        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace,
                "source_schema_id": source["schema"]["id"]
            }))
            .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let fork: serde_json::Value = created.json();
        let fork_id = fork["id"].as_str().unwrap().parse::<Uuid>().unwrap();
        let original = workspace_schema_forks::Entity::find_by_id(fork_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        let tenant_b = tenant_tenants::ActiveModel {
            name: ActiveValue::Set("sqlite-tenant-b".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();

        let update = workspace_schema_forks::Entity::update_many()
            .col_expr(
                workspace_schema_forks::Column::TenantId,
                sea_orm::sea_query::Expr::value(tenant_b.id),
            )
            .filter(workspace_schema_forks::Column::Id.eq(fork_id))
            .exec(&ctx.db)
            .await;
        assert!(
            update.is_err(),
            "SQLite trigger must reject mismatched UPDATE"
        );

        let insert = workspace_schema_forks::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_b.id),
            workspace_id: ActiveValue::Set(target.id),
            source_workspace_id: ActiveValue::Set(original.source_workspace_id),
            source_schema_id: ActiveValue::Set(original.source_schema_id),
            source_schema_version: ActiveValue::Set(original.source_schema_version),
            source_schema_name: ActiveValue::Set(original.source_schema_name),
            fork_schema_id: ActiveValue::Set(original.fork_schema_id),
            customized: ActiveValue::Set(false),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await;
        assert!(
            insert.is_err(),
            "SQLite trigger must reject mismatched INSERT"
        );
    })
    .await;
}
