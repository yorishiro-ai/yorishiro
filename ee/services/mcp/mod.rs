mod origin;

use rmcp::handler::server::router::tool::ToolRouter;

use crate::services::mcp::YorishiroMcpServer;
use crate::services::mcp::compose_tool_routers;

/// The complete enterprise-only MCP tool set.
pub(crate) fn tool_router() -> ToolRouter<YorishiroMcpServer> {
    compose_tool_routers([YorishiroMcpServer::tool_router_origin()])
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::tool_router;
    use crate::services::mcp::{community_tool_router, render_inventory_fragment};

    #[test]
    fn enterprise_inventory_contract_is_disjoint_and_contains_community() {
        let community = community_tool_router().list_all();
        let enterprise = tool_router().list_all();
        let full =
            crate::services::mcp::compose_tool_routers([community_tool_router(), tool_router()])
                .list_all();
        let community_names: HashSet<_> = community.iter().map(|tool| &tool.name).collect();
        let enterprise_names: HashSet<_> = enterprise.iter().map(|tool| &tool.name).collect();
        let full_names: HashSet<_> = full.iter().map(|tool| &tool.name).collect();
        let names: Vec<_> = enterprise.iter().map(|tool| tool.name.as_ref()).collect();
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(
            names, sorted_names,
            "enterprise MCP tool names are not sorted"
        );
        assert!(community_names.is_disjoint(&enterprise_names));
        let expected_full_names: HashSet<_> =
            community_names.union(&enterprise_names).cloned().collect();
        assert_eq!(full_names, expected_full_names);
        assert!(community_names.is_subset(&full_names));
        assert!(enterprise_names.is_subset(&full_names));
        for tool in &enterprise {
            assert!(
                tool.description
                    .as_deref()
                    .is_some_and(|description| !description.trim().is_empty()),
                "enterprise tool {} has no description",
                tool.name
            );
            let schema = tool.schema_as_json_value();
            assert_eq!(schema["type"], "object");
            assert!(jsonschema::meta::options().is_valid(&schema));
        }
        println!("enterprise MCP tools: {sorted_names:?}");

        let docs = include_str!("../../../docs/en/mcp-tools.md");
        let start = "<!-- BEGIN GENERATED MCP INVENTORY -->\n";
        let end = "<!-- END GENERATED MCP INVENTORY -->";
        let generated = docs
            .split_once(start)
            .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
            .expect("English MCP documentation markers");
        assert_eq!(
            generated,
            render_inventory_fragment(&community, &enterprise)
        );
    }
}
