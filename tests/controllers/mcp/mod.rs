use crate::controllers::render_inventory_section;
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
    fn entity_rest_and_mcp_adapters_build_equivalent_inputs() {
        let rest: entity_entities::CreateEntityInput = rest_entities::CreateEntityRequest {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: json!({"title": "hello"}),
        }
        .into();
        let mcp: entity_entities::CreateEntityInput = entities::CreateEntityArgs {
            schema_name: "notes".into(),
            entity_type: "note".into(),
            data: json!({"title": "hello"}),
        }
        .into();

        assert_eq!(rest.schema_name, mcp.schema_name);
        assert_eq!(rest.entity_type, mcp.entity_type);
        assert_eq!(rest.data, mcp.data);

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
    fn relation_rest_and_mcp_adapters_build_equivalent_inputs() {
        let source_id = Uuid::now_v7();
        let target_id = Uuid::now_v7();
        let rest: entity_relations::CreateRelationInput = rest_relations::CreateRelationRequest {
            source_id,
            target_id,
            relation_type: "knows".into(),
            properties: None,
        }
        .into();
        let mcp: entity_relations::CreateRelationInput = relations::CreateRelationArgs {
            source_id,
            target_id,
            relation_type: "knows".into(),
            properties: None,
        }
        .into();

        assert_eq!(rest.source_id, mcp.source_id);
        assert_eq!(rest.target_id, mcp.target_id);
        assert_eq!(rest.relation_type, mcp.relation_type);
        assert_eq!(rest.properties, mcp.properties);

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
            .into();
        let mcp_query: entity_relations::ListRelationsQuery = relations::ListRelationsArgs {
            source_id: Some(source_id),
            target_id: Some(target_id),
            relation_type: Some("knows".into()),
            status: Some("deprecated".into()),
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

    #[test]
    fn mcp_closed_relation_values_fail_before_model_query_conversion() {
        let result: Result<entity_relations::ListRelationsQuery, _> =
            relations::ListRelationsArgs {
                source_id: None,
                target_id: None,
                relation_type: None,
                status: Some("unknown".into()),
                limit: None,
                offset: None,
            }
            .try_into();

        match result {
            Err(yorishiro::YorishiroError::ValidationFailed { .. }) => {}
            Ok(_) => panic!("invalid status must be rejected"),
            Err(error) => panic!("unexpected validation error: {error:?}"),
        }
    }
}

use rmcp::handler::server::router::tool::ToolRoute;

use yorishiro::controllers::mcp::{community_tool_router, compose_tool_routers};

fn assert_inventory_contract(label: &str, tools: &[rmcp::model::Tool]) {
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
fn community_inventory_contract_and_documentation_are_current() {
    let community = community_tool_router().list_all();
    assert_inventory_contract("community", &community);

    let docs = include_str!("../../../docs/en/mcp-tools.md");
    let start = "<!-- BEGIN GENERATED MCP INVENTORY -->\n";
    let end = "## Enterprise-only tools\n";
    let generated_community = docs
        .split_once(start)
        .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
        .expect("English MCP community inventory markers");
    assert_eq!(
        generated_community,
        render_inventory_section("Community tools", &community),
        "English MCP community documentation has drifted from the runtime inventory"
    );
}

#[test]
fn composition_rejects_duplicate_registered_names_even_when_disabled() {
    fn router(name: &'static str) -> ToolRouter<yorishiro::controllers::mcp::YorishiroMcpServer> {
        ToolRouter::new().with_route(ToolRoute::new_dyn(
            Tool::new(name, "test tool", Arc::new(Default::default())),
            |_context| Box::pin(async { Ok(CallToolResult::default().into()) }),
        ))
    }

    let disabled = router("duplicate").with_disabled("duplicate");
    assert!(disabled.list_all().is_empty());
    assert!(disabled.map.contains_key("duplicate"));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compose_tool_routers([router("duplicate"), disabled]);
    }));
    assert!(
        result.is_err(),
        "duplicate registered names must be rejected"
    );
}
