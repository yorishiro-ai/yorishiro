use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::{content_entities, content_relations, content_schemas, recall};

fn chain_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "chain",
        "entity_types": {
            "node": { "fields": { "title": { "type": "string", "required": true, "x-embed": true } } }
        },
        "relation_types": {
            "links_to": { "source": "node", "target": "node" }
        }
    })
}

/// `recall_context` at `depth: 2` must reach a neighbor-of-neighbor (root -> mid -> leaf) but not go further, and a diamond back to an already-visited node must not be reported twice or re-expanded.
#[tokio::test]
#[serial]
async fn recall_context_traverses_two_hops_and_dedupes_a_diamond() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("recall-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");
        let def = serde_json::from_value(chain_definition()).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let make_node = |title: &str| {
            content_entities::CreateEntityInput {
                schema_name: "chain".into(),
                entity_type: "node".into(),
                data: serde_json::json!({ "title": title }),
            }
        };

        let root = content_entities::create(&ctx.db, workspace.id, make_node("root"), None)
            .await
            .expect("create root");
        let mid = content_entities::create(&ctx.db, workspace.id, make_node("mid"), None)
            .await
            .expect("create mid");
        let leaf = content_entities::create(&ctx.db, workspace.id, make_node("leaf"), None)
            .await
            .expect("create leaf");
        // Not reachable within depth 2 from root (root -> mid -> leaf -> unreached is 3 hops).
        let unreached =
            content_entities::create(&ctx.db, workspace.id, make_node("unreached"), None)
                .await
                .expect("create unreached");

        content_relations::create(
            &ctx.db,
            workspace.id,
            content_relations::CreateRelationInput {
                source_id: root.id,
                target_id: mid.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("root -> mid");
        content_relations::create(
            &ctx.db,
            workspace.id,
            content_relations::CreateRelationInput {
                source_id: mid.id,
                target_id: leaf.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("mid -> leaf");
        // Diamond: leaf also points straight back to mid, so mid is reachable via two paths.
        content_relations::create(
            &ctx.db,
            workspace.id,
            content_relations::CreateRelationInput {
                source_id: leaf.id,
                target_id: mid.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("leaf -> mid (diamond)");
        content_relations::create(
            &ctx.db,
            workspace.id,
            content_relations::CreateRelationInput {
                source_id: leaf.id,
                target_id: unreached.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("leaf -> unreached");

        let context = recall::recall_context(
            &ctx.db,
            workspace.id,
            root.id,
            recall::RecallQuery {
                limit: 20,
                full: true,
                depth: 2,
            },
        )
        .await
        .expect("recall_context");

        assert_eq!(context.entity.id, root.id);
        let ids: Vec<_> = context.relations.iter().map(|r| r.neighbor.id).collect();
        assert!(ids.contains(&mid.id), "mid must be reached: {ids:?}");
        assert!(ids.contains(&leaf.id), "leaf must be reached at hop 2: {ids:?}");
        assert!(
            !ids.contains(&unreached.id),
            "unreached is 3 hops away, past depth 2: {ids:?}"
        );
        assert_eq!(
            ids.iter().filter(|&&id| id == mid.id).count(),
            1,
            "mid reached via two paths (direct + diamond back from leaf) must be reported once: {ids:?}"
        );
        let mid_relation = context
            .relations
            .iter()
            .find(|r| r.neighbor.id == mid.id)
            .expect("mid relation present");
        assert_eq!(
            mid_relation.hop_distance, 1,
            "mid must keep its shortest hop_distance, not the diamond's longer one"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}

/// `full: false` (the default) must reduce a neighbor's `data` to only its `x-embed` fields.
#[tokio::test]
#[serial]
async fn recall_context_shallow_copy_keeps_only_x_embed_fields() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let tenant = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("recall-shallow-test".into()),
            ..Default::default()
        };
        let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
            .await
            .expect("insert tenant");
        let workspace = identity_workspaces::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant.id),
            name: sea_orm::ActiveValue::Set("main".into()),
            status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
            ..Default::default()
        };
        let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
            .await
            .expect("insert workspace");

        let def_json = serde_json::json!({
            "name": "person",
            "entity_types": {
                "person": {
                    "fields": {
                        "name": { "type": "string", "required": true, "x-embed": true },
                        "ssn": { "type": "string", "required": false }
                    }
                }
            },
            "relation_types": { "knows": { "source": "person", "target": "person" } }
        });
        let def = serde_json::from_value(def_json).expect("parse definition");
        content_schemas::create_schema(&ctx.db, tenant.id, workspace.id, def, None, None)
            .await
            .expect("create schema");

        let alice = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "person".into(),
                entity_type: "person".into(),
                data: serde_json::json!({ "name": "Alice" }),
            },
            None,
        )
        .await
        .expect("create alice");
        let bob = content_entities::create(
            &ctx.db,
            workspace.id,
            content_entities::CreateEntityInput {
                schema_name: "person".into(),
                entity_type: "person".into(),
                data: serde_json::json!({ "name": "Bob", "ssn": "secret" }),
            },
            None,
        )
        .await
        .expect("create bob");
        content_relations::create(
            &ctx.db,
            workspace.id,
            content_relations::CreateRelationInput {
                source_id: alice.id,
                target_id: bob.id,
                relation_type: "knows".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("alice knows bob");

        let context = recall::recall_context(
            &ctx.db,
            workspace.id,
            alice.id,
            recall::RecallQuery::default(),
        )
        .await
        .expect("recall_context");

        assert_eq!(context.relations.len(), 1);
        let bob_data = &context.relations[0].neighbor.data;
        assert_eq!(bob_data.get("name").unwrap(), "Bob");
        assert!(
            bob_data.get("ssn").is_none(),
            "non-x-embed field must be stripped by shallow_copy: {bob_data:?}"
        );

        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}
