use crate::requests::boot_request;
use crate::requests::fixtures::{self, TenantArgs, TestTenant};
use axum::http::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::_entities::{
    entity_entities, tenant_tenants, workspace_schema_fork_heads, workspace_schema_forks,
};

// Fork heads are archived, so normal entity creation cannot reference them.
// Inject an existing historical reference to exercise deletion's integrity guard.
// Direct write (a): no operation creates an entity on an archived schema version.
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
#[expect(
    clippy::too_many_lines,
    reason = "one fork history covers copy and follow modes and checks that recorded heads cannot be rewritten"
)]
async fn schema_fork_crud_copy_follow_and_history_guards() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id: source_workspace_id,
            owner_id,
            api_key: source_key,
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let schema_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .add_header("x-workspace-id", source_workspace_id.to_string())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(schema_response.status_code(), StatusCode::CREATED);
        let schema: serde_json::Value = schema_response.json();
        let source_schema_id: Uuid = schema["schema"]["schema_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let target = fixtures::create_workspace(&ctx, tenant_id, "target").await;
        let target_key = fixtures::issue_access_key(&ctx, target.id, owner_id).await;
        let read_key = fixtures::issue_access_key(
            &ctx,
            target.id,
            fixtures::create_iam_reader(&ctx, tenant_id).await,
        )
        .await;

        let denied = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(denied.status_code(), StatusCode::FORBIDDEN);

        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(
            created.status_code(),
            StatusCode::CREATED,
            "{}",
            created.text()
        );
        let fork: serde_json::Value = created.json();
        assert_eq!(
            fork["source_schema_id"].as_str().unwrap(),
            source_schema_id.to_string()
        );
        assert_eq!(fork["customized"], false);
        assert_eq!(fork["source_schema_version"], schema["schema"]["version"]);
        assert_eq!(fork["definition"], schema["schema"]["definition"]);
        let fork_id = fork["fork_id"].as_str().unwrap().to_owned();
        let original_head_id: Uuid = fork["fork_schema_id"].as_str().unwrap().parse().unwrap();
        let original_head_version =
            i32::try_from(fork["fork_schema_version"].as_i64().unwrap()).unwrap();
        // Direct write (b): proves the fork-head history trigger rejects updates.
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
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed: serde_json::Value = listed.json();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["fork_id"], fork["fork_id"]);

        let fetched = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(fetched.status_code(), StatusCode::OK);
        assert_eq!(
            fetched.json::<serde_json::Value>()["definition"],
            fork["definition"]
        );

        let duplicate = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace_id,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(duplicate.status_code(), StatusCode::CONFLICT);

        let local = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "definition": fork["definition"],
                "expected_fork_schema_id": fork["fork_schema_id"]
            }))
            .await;
        assert_eq!(local.status_code(), StatusCode::OK, "{}", local.text());
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

        let mut renamed_definition = local["definition"].clone();
        renamed_definition["name"] = serde_json::json!("renamed-fork");
        let history_before_rename = workspace_schema_fork_heads::Entity::find()
            .filter(
                workspace_schema_fork_heads::Column::ForkId.eq(fork_id.parse::<Uuid>().unwrap()),
            )
            .count(&ctx.db)
            .await
            .unwrap();
        let renamed = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "definition": renamed_definition,
                "expected_fork_schema_id": local["fork_schema_id"]
            }))
            .await;
        assert_eq!(
            renamed.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            renamed.text()
        );
        assert_eq!(
            renamed.json::<serde_json::Value>()["error"]["code"],
            "validation_failed"
        );
        let fork_after_rename = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(fork_after_rename.status_code(), StatusCode::OK);
        let fork_after_rename: serde_json::Value = fork_after_rename.json();
        assert_eq!(fork_after_rename["fork_schema_id"], local["fork_schema_id"]);
        assert_eq!(
            workspace_schema_fork_heads::Entity::find()
                .filter(
                    workspace_schema_fork_heads::Column::ForkId
                        .eq(fork_id.parse::<Uuid>().unwrap()),
                )
                .count(&ctx.db)
                .await
                .unwrap(),
            history_before_rename
        );

        let stale = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "definition": local["definition"],
                "expected_fork_schema_id": original_head_id
            }))
            .await;
        assert_eq!(stale.status_code(), StatusCode::CONFLICT);

        let source_v2 = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .add_header("x-workspace-id", source_workspace_id.to_string())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(
            source_v2.status_code(),
            StatusCode::CREATED,
            "{}",
            source_v2.text()
        );
        let source_v2: serde_json::Value = source_v2.json();
        let source_v2_id = source_v2["schema"]["schema_id"].as_str().unwrap();

        let before_follow_get = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(before_follow_get.status_code(), StatusCode::OK);
        assert_eq!(
            before_follow_get.json::<serde_json::Value>()["upstream_version"],
            source_v2["schema"]["version"]
        );
        let before_follow_list = request
            .get("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(before_follow_list.status_code(), StatusCode::OK);
        assert_eq!(
            before_follow_list.json::<serde_json::Value>()[0]["upstream_version"],
            source_v2["schema"]["version"]
        );

        let follow = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "action": "follow",
                "expected_fork_schema_id": local["fork_schema_id"],
                "expected_source_schema_id": source_v2_id
            }))
            .await;
        assert_eq!(follow.status_code(), StatusCode::CONFLICT);

        let followed = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "action": "follow",
                "force": true,
                "expected_fork_schema_id": local["fork_schema_id"],
                "expected_source_schema_id": source_v2_id
            }))
            .await;
        assert_eq!(
            followed.status_code(),
            StatusCode::OK,
            "{}",
            followed.text()
        );
        let followed: serde_json::Value = followed.json();
        assert_eq!(followed["customized"], false);
        assert_eq!(followed["source_schema_id"], source_v2_id);
        assert_eq!(followed["definition"], source_v2["schema"]["definition"]);
        let after_follow_get = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(after_follow_get.status_code(), StatusCode::OK);
        assert!(after_follow_get.json::<serde_json::Value>()["upstream_version"].is_null());
        let after_follow_list = request
            .get("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(after_follow_list.status_code(), StatusCode::OK);
        assert!(after_follow_list.json::<serde_json::Value>()[0]["upstream_version"].is_null());
        let current_head_id: Uuid =
            serde_json::from_value(followed["fork_schema_id"].clone()).unwrap();
        let current_head_version =
            i32::try_from(followed["fork_schema_version"].as_i64().unwrap()).unwrap();
        let old_reference =
            insert_reference(&ctx, target.id, original_head_id, original_head_version).await;
        let delete_old_referenced = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(delete_old_referenced.status_code(), StatusCode::CONFLICT);
        yorishiro::models::entity_entities::delete(&ctx.db, target.id, old_reference)
            .await
            .unwrap();

        let current_reference =
            insert_reference(&ctx, target.id, current_head_id, current_head_version).await;
        let delete_current_referenced = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(
            delete_current_referenced.status_code(),
            StatusCode::CONFLICT
        );
        yorishiro::models::entity_entities::delete(&ctx.db, target.id, current_reference)
            .await
            .unwrap();

        let deleted = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(deleted.status_code(), StatusCode::NO_CONTENT);

        let missing = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(missing.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(
            workspace_schema_fork_heads::Entity::find()
                .filter(
                    workspace_schema_fork_heads::Column::ForkId.eq(fork["fork_id"]
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
        for head_id in [original_head_id, current_head_id] {
            let preserved = request
                .get(&format!("/api/schemas/{head_id}"))
                .add_header("Authorization", format!("Bearer {target_key}"))
                .add_header("x-workspace-id", target.id.to_string())
                .await;
            assert_eq!(
                preserved.status_code(),
                StatusCode::OK,
                "{}",
                preserved.text()
            );
            assert_eq!(
                preserved.json::<serde_json::Value>()["schema_id"],
                head_id.to_string()
            );
        }
    })
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "two tenant fixtures exercise the same fork through each HTTP path to verify RLS isolation"
)]
async fn schema_fork_http_routes_use_rls_for_cross_tenant_isolation() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_multitenant_request(|request, ctx| async move {
        let TestTenant {
            tenant_id: tenant_a,
            workspace_id: source_workspace,
            owner_id: owner_a,
            api_key: source_key,
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                tenant_name: "fork-http-tenant-a".into(),
                owner_email: fixtures::unique_email("fork-http-a"),
                ..Default::default()
            },
        )
        .await;
        let source_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .add_header("x-workspace-id", source_workspace.to_string())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(source_response.status_code(), StatusCode::CREATED);
        let source: serde_json::Value = source_response.json();
        let source_schema_id = source["schema"]["schema_id"].as_str().unwrap();
        let target = fixtures::create_workspace(&ctx, tenant_a, "fork-http-target").await;
        let target_key = fixtures::issue_access_key(&ctx, target.id, owner_a).await;
        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(
            created.status_code(),
            StatusCode::CREATED,
            "{}",
            created.text()
        );
        let fork: serde_json::Value = created.json();
        let fork_id = fork["fork_id"].as_str().unwrap();

        let TestTenant {
            workspace_id: workspace_b,
            owner_id: owner_b,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                tenant_name: "fork-http-tenant-b".into(),
                workspace_name: "fork-http-b".into(),
                owner_email: fixtures::unique_email("fork-http-b"),
                ..Default::default()
            },
        )
        .await;
        let read_b = fixtures::issue_access_key(&ctx, workspace_b, owner_b).await;
        let schema_b = fixtures::issue_access_key(&ctx, workspace_b, owner_b).await;

        let list_b = request
            .get("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {read_b}"))
            .add_header("x-workspace-id", workspace_b.to_string())
            .await;
        assert_eq!(list_b.status_code(), StatusCode::OK);
        assert_eq!(list_b.json::<serde_json::Value>(), serde_json::json!([]));

        let get_b = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {read_b}"))
            .add_header("x-workspace-id", workspace_b.to_string())
            .await;
        assert_eq!(get_b.status_code(), StatusCode::NOT_FOUND);

        let create_cross_tenant = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {schema_b}"))
            .add_header("x-workspace-id", workspace_b.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(create_cross_tenant.status_code(), StatusCode::NOT_FOUND);

        let update_cross_tenant = request
            .put(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {schema_b}"))
            .add_header("x-workspace-id", workspace_b.to_string())
            .json(&serde_json::json!({ "action": "follow" }))
            .await;
        assert_eq!(update_cross_tenant.status_code(), StatusCode::NOT_FOUND);

        let delete_cross_tenant = request
            .delete(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {schema_b}"))
            .add_header("x-workspace-id", workspace_b.to_string())
            .await;
        assert_eq!(delete_cross_tenant.status_code(), StatusCode::NOT_FOUND);

        let still_there = request
            .get(&format!("/api/schema-forks/{fork_id}"))
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .await;
        assert_eq!(still_there.status_code(), StatusCode::OK);
    })
    .await;
}

#[tokio::test]
async fn schema_forks_reject_transitive_workspace_cycles() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id: workspace_a,
            owner_id,
            api_key: key_a,
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let workspace_b = fixtures::create_workspace(&ctx, tenant_id, "workspace-b").await;
        let workspace_c = fixtures::create_workspace(&ctx, tenant_id, "workspace-c").await;
        let key_b = fixtures::issue_access_key(&ctx, workspace_b.id, owner_id).await;
        let key_c = fixtures::issue_access_key(&ctx, workspace_c.id, owner_id).await;

        let mut schemas = Vec::new();
        for (key, workspace) in [
            (&key_a, workspace_a),
            (&key_b, workspace_b.id),
            (&key_c, workspace_c.id),
        ] {
            let response = request
                .post("/api/schemas")
                .add_header("Authorization", format!("Bearer {key}"))
                .add_header("x-workspace-id", workspace.to_string())
                .json(&serde_json::json!({ "template_id": "task-management" }))
                .await;
            assert_eq!(
                response.status_code(),
                StatusCode::CREATED,
                "{}",
                response.text()
            );
            let body: serde_json::Value = response.json();
            schemas.push(
                body["schema"]["schema_id"]
                    .as_str()
                    .unwrap()
                    .parse::<Uuid>()
                    .unwrap(),
            );
        }

        let b_follows_a = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {key_b}"))
            .add_header("x-workspace-id", workspace_b.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": workspace_a,
                "source_schema_id": schemas[0]
            }))
            .await;
        assert_eq!(
            b_follows_a.status_code(),
            StatusCode::CREATED,
            "{}",
            b_follows_a.text()
        );

        let c_follows_b = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {key_c}"))
            .add_header("x-workspace-id", workspace_c.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": workspace_b.id,
                "source_schema_id": schemas[1]
            }))
            .await;
        assert_eq!(
            c_follows_b.status_code(),
            StatusCode::CREATED,
            "{}",
            c_follows_b.text()
        );

        let cycle = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {key_a}"))
            .add_header("x-workspace-id", workspace_a.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": workspace_c.id,
                "source_schema_id": schemas[2]
            }))
            .await;
        assert_eq!(
            cycle.status_code(),
            StatusCode::CONFLICT,
            "{}",
            cycle.text()
        );
    })
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "both direct write attempts use the same scoped tenant and foreign fork to verify isolation and schema integrity"
)]
async fn schema_fork_rls_and_integrity_reject_cross_tenant_rows() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_multitenant_request(|request, ctx| async move {
        let TestTenant {
            tenant_id: tenant_a,
            workspace_id: source_workspace,
            owner_id: owner_a,
            api_key: source_key,
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                tenant_name: "tenant-a".into(),
                owner_email: fixtures::unique_email("owner-a"),
                ..Default::default()
            },
        )
        .await;
        let source_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .add_header("x-workspace-id", source_workspace.to_string())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        let source: serde_json::Value = source_response.json();
        let source_schema_id = source["schema"]["schema_id"].as_str().unwrap();
        let target = fixtures::create_workspace(&ctx, tenant_a, "target-a").await;
        let target_key = fixtures::issue_access_key(&ctx, target.id, owner_a).await;
        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace,
                "source_schema_id": source_schema_id
            }))
            .await;
        assert_eq!(
            created.status_code(),
            StatusCode::CREATED,
            "{}",
            created.text()
        );
        let fork: serde_json::Value = created.json();
        let fork_id = fork["fork_id"].as_str().unwrap().parse::<Uuid>().unwrap();

        let TestTenant {
            tenant_id: tenant_b,
            workspace_id: workspace_b,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                tenant_name: "tenant-b".into(),
                workspace_name: "workspace-b".into(),
                owner_email: fixtures::unique_email("owner-b"),
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
        // Direct write (b): proves RLS rejects a cross-tenant update.
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
        // Direct write (b): proves RLS rejects a cross-tenant insert.
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
async fn sqlite_schema_fork_integrity_rejects_cross_tenant_rows() {
    if !crate::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id: tenant_a,
            workspace_id: source_workspace,
            owner_id,
            api_key: source_key,
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let source_response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {source_key}"))
            .add_header("x-workspace-id", source_workspace.to_string())
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        let source: serde_json::Value = source_response.json();
        let target = fixtures::create_workspace(&ctx, tenant_a, "sqlite-target").await;
        let target_key = fixtures::issue_access_key(&ctx, target.id, owner_id).await;
        let created = request
            .post("/api/schema-forks")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .add_header("x-workspace-id", target.id.to_string())
            .json(&serde_json::json!({
                "source_workspace_id": source_workspace,
                "source_schema_id": source["schema"]["schema_id"]
            }))
            .await;
        assert_eq!(
            created.status_code(),
            StatusCode::CREATED,
            "{}",
            created.text()
        );
        let fork: serde_json::Value = created.json();
        let fork_id = fork["fork_id"].as_str().unwrap().parse::<Uuid>().unwrap();
        let original = workspace_schema_forks::Entity::find_by_id(fork_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();

        // Direct writes below (b): bypass the one-tenant policy to exercise the database's cross-tenant integrity trigger.
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
            id: ActiveValue::Set(Uuid::new_v4()),
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
        let insert_error = insert
            .expect_err("SQLite trigger must reject mismatched INSERT")
            .to_string();
        assert!(
            insert_error.contains("workspace_schema_forks target workspace mismatch"),
            "unexpected SQLite integrity error: {insert_error}"
        );
    })
    .await;
}
