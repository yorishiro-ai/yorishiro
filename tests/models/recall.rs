use std::time::Duration;

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use yorishiro::App;
use yorishiro::models::{entity_entities, entity_relations, recall, schema_schemas};

/// A relation-free entity at depth two must return its own body and no relations promptly on both backends.
#[tokio::test]
async fn recall_context_depth_two_no_relations_returns_empty_without_hanging() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let def = serde_json::from_value(chain_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            def,
            None,
            None,
        )
        .await
        .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "chain".into(),
                entity_type: "node".into(),
                data: serde_json::json!({ "title": "isolated" }),
            },
            None,
        )
        .await
        .expect("create relation-free entity");

        let context = tokio::time::timeout(
            Duration::from_secs(5),
            recall::recall_context(
                &ctx.db,
                fixture.workspace_id,
                entity.id,
                recall::RecallQuery {
                    limit: 20,
                    full: true,
                    depth: 2,
                },
            ),
        )
        .await
        .expect("depth-two recall of relation-free entity timed out")
        .expect("recall_context");

        assert_eq!(context.entity.id, entity.id);
        assert!(
            context.relations.is_empty(),
            "relation-free recall must be empty"
        );
    })
    .await;
}

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
async fn recall_context_traverses_two_hops_and_dedupes_a_diamond() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let def = serde_json::from_value(chain_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, fixture.tenant_id, fixture.workspace_id, def, None, None)
            .await
            .expect("create schema");

        let make_node = |title: &str| {
            entity_entities::CreateEntityInput {
                schema_name: "chain".into(),
                entity_type: "node".into(),
                data: serde_json::json!({ "title": title }),
            }
        };

        let root = entity_entities::create(&ctx.db, fixture.workspace_id, make_node("root"), None)
            .await
            .expect("create root");
        let mid = entity_entities::create(&ctx.db, fixture.workspace_id, make_node("mid"), None)
            .await
            .expect("create mid");
        let leaf = entity_entities::create(&ctx.db, fixture.workspace_id, make_node("leaf"), None)
            .await
            .expect("create leaf");
        // Not reachable within depth 2 from root (root -> mid -> leaf -> unreached is 3 hops).
        let unreached =
            entity_entities::create(&ctx.db, fixture.workspace_id, make_node("unreached"), None)
                .await
                .expect("create unreached");

        entity_relations::create(
            &ctx.db,
            fixture.workspace_id,
            entity_relations::CreateRelationInput {
                source_id: root.id,
                target_id: mid.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("root -> mid");
        entity_relations::create(
            &ctx.db,
            fixture.workspace_id,
            entity_relations::CreateRelationInput {
                source_id: mid.id,
                target_id: leaf.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("mid -> leaf");
        // Diamond: leaf also points straight back to mid, so mid is reachable via two paths.
        entity_relations::create(
            &ctx.db,
            fixture.workspace_id,
            entity_relations::CreateRelationInput {
                source_id: leaf.id,
                target_id: mid.id,
                relation_type: "links_to".into(),
                properties: serde_json::json!({}),
            },
        )
        .await
        .expect("leaf -> mid (diamond)");
        entity_relations::create(
            &ctx.db,
            fixture.workspace_id,
            entity_relations::CreateRelationInput {
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
            fixture.workspace_id,
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

        let bounded = recall::recall_context(
            &ctx.db, fixture.workspace_id, root.id,
            recall::RecallQuery { limit: 1, full: true, depth: 2 },
        ).await.expect("per-pivot bounded recall");
        assert_eq!(serde_json::to_value(&bounded).unwrap()["truncated"], true, "the mid pivot has more than one relation");
        assert_eq!(bounded.relations.len(), 2, "each hop retains its own limit, not a shared batch limit");

    })
    .await;
}

/// `full: false` (the default) must reduce a neighbor's `data` to only its `x-embed` fields.
#[tokio::test]
async fn recall_context_shallow_copy_keeps_only_x_embed_fields() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

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
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            def,
            None,
            None,
        )
        .await
        .expect("create schema");

        let alice = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "person".into(),
                entity_type: "person".into(),
                data: serde_json::json!({ "name": "Alice" }),
            },
            None,
        )
        .await
        .expect("create alice");
        let bob = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "person".into(),
                entity_type: "person".into(),
                data: serde_json::json!({ "name": "Bob", "ssn": "secret" }),
            },
            None,
        )
        .await
        .expect("create bob");
        entity_relations::create(
            &ctx.db,
            fixture.workspace_id,
            entity_relations::CreateRelationInput {
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
            fixture.workspace_id,
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
    })
    .await;
}
