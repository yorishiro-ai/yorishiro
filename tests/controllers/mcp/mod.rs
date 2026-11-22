use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::CallToolResult;
use rmcp::model::Tool;
use std::collections::HashSet;
use std::sync::Arc;
mod input_parity_tests {
    use serde_json::json;
    use uuid::Uuid;
    use yorishiro::controllers::mcp::*;
    use yorishiro::dtos::{entities as rest_entities, relations as rest_relations};
    use yorishiro::models::{entity_entities, entity_relations};

    #[test]
    fn entity_list_rest_and_mcp_adapters_build_equivalent_queries() {
        let rest_query: entity_entities::ListEntitiesQuery = rest_entities::ListEntitiesParams {
            entity_type: Some("note".into()),
            filter: Some(r#"{"title":"hello"}"#.into()),
            schema_version: Some(2),
            page: yorishiro::dtos::common::PageParams(loco_rs::model::query::PaginationQuery {
                page: 3,
                page_size: 7,
            }),
        }
        .try_into()
        .expect("REST entity query");
        let mcp_query: entity_entities::ListEntitiesQuery = entities::ListEntitiesArgs {
            entity_type: Some("note".into()),
            filter: Some(json!({"title": "hello"})),
            schema_version: Some(2),
            limit: Some(7),
            offset: Some(14),
        }
        .try_into()
        .expect("MCP entity query");

        assert_eq!(rest_query.entity_type, mcp_query.entity_type);
        assert_eq!(rest_query.filter, mcp_query.filter);
        assert_eq!(rest_query.schema_version, mcp_query.schema_version);
        assert_eq!(rest_query.page.limit(), mcp_query.page.limit());
        assert_eq!(rest_query.page.offset(), mcp_query.page.offset());
    }

    #[test]
    fn relation_list_rest_and_mcp_adapters_build_equivalent_queries() {
        let source_id = Uuid::now_v7();
        let target_id = Uuid::now_v7();
        let rest_query: entity_relations::ListRelationsQuery =
            rest_relations::ListRelationsParams {
                source_id: Some(source_id),
                target_id: Some(target_id),
                relation_type: Some("knows".into()),
                status: Some(entity_relations::RelationStatus::Deprecated),
                page: yorishiro::dtos::common::PageParams(loco_rs::model::query::PaginationQuery {
                    page: 2,
                    page_size: 5,
                }),
            }
            .try_into()
            .expect("REST relation query");
        let mcp_query: entity_relations::ListRelationsQuery = relations::ListRelationsArgs {
            source_id: Some(source_id),
            target_id: Some(target_id),
            relation_type: Some("knows".into()),
            status: Some(entity_relations::RelationStatus::Deprecated),
            limit: Some(5),
            offset: Some(5),
        }
        .try_into()
        .expect("MCP relation query");

        assert_eq!(rest_query.source_id, mcp_query.source_id);
        assert_eq!(rest_query.target_id, mcp_query.target_id);
        assert_eq!(rest_query.relation_type, mcp_query.relation_type);
        assert_eq!(rest_query.status, mcp_query.status);
        assert_eq!(rest_query.page.limit(), mcp_query.page.limit());
        assert_eq!(rest_query.page.offset(), mcp_query.page.offset());
    }
}

use rmcp::handler::server::router::tool::ToolRoute;

use yorishiro::controllers::mcp::{community_tool_router, compose_tool_routers};

fn assert_inventory_contract(label: &str, tools: &[Tool]) {
    let names: Vec<_> = tools.iter().map(|tool| tool.name.to_string()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "{label} MCP tool names are not sorted: {names:?}"
    );
    let unique: HashSet<_> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "duplicate {label} MCP tool names: {names:?}"
    );
    for tool in tools {
        let required = tool
            .meta
            .as_ref()
            .and_then(|meta| meta.0.get("yorishiro/required_scope"))
            .expect("every registered tool declares its authorization scope");
        serde_json::from_value::<yorishiro::models::api_keys::ApiKeyScope>(required.clone())
            .expect("tool scope is valid");
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|description| !description.trim().is_empty()),
            "{label} tool {} has no description",
            tool.name
        );
        let schema = tool.schema_as_json_value();
        assert_eq!(
            schema["type"], "object",
            "invalid {label} schema for {}",
            tool.name
        );
        assert!(
            jsonschema::meta::options().is_valid(&schema),
            "invalid {label} input schema for {}: {schema}",
            tool.name
        );
    }
}

#[test]
fn community_inventory_contract_is_valid() {
    let community = community_tool_router().list_all();
    assert_inventory_contract("community", &community);
}

#[test]
fn composition_rejects_duplicate_registered_names_even_when_disabled() {
    fn router(name: &'static str) -> ToolRouter<yorishiro::controllers::mcp::YorishiroMcpServer> {
        ToolRouter::new().with_route(ToolRoute::new_dyn(
            Tool::new(name, "test tool", Arc::new(serde_json::Map::default())),
            |_context| Box::pin(async { Ok(CallToolResult::default().into()) }),
        ))
    }

    let disabled = router("duplicate").with_disabled("duplicate");
    assert_eq!(disabled.list_all(), [] as [Tool; 0]);
    assert!(disabled.map.contains_key("duplicate"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_tool_routers([router("duplicate"), disabled]);
    }));
    assert!(
        result.is_err(),
        "duplicate registered names must be rejected"
    );
}
