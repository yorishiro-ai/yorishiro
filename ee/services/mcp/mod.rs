mod origin;

use rmcp::handler::server::router::tool::ToolRouter;

use crate::services::mcp::YorishiroMcpServer;

/// The complete enterprise-only MCP tool set.
pub(crate) fn tool_router() -> ToolRouter<YorishiroMcpServer> {
    YorishiroMcpServer::tool_router_origin()
}

#[cfg(test)]
mod tests {
    use super::tool_router;
    use crate::services::mcp::community_tool_router;

    #[test]
    fn enterprise_tool_inventory_is_exact() {
        let names: Vec<_> = (community_tool_router() + tool_router())
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(
            names,
            [
                "create_entity",
                "create_relation",
                "create_schema",
                "delete_entity",
                "delete_relation",
                "fill_defaults",
                "get_active_schema",
                "get_entity",
                "get_entity_drift",
                "get_entity_type_json_schema",
                "get_relation",
                "get_schema_by_id",
                "get_template_library_item",
                "import_jsonl",
                "list_entities",
                "list_relations",
                "list_schemas",
                "list_template_library",
                "list_templates",
                "list_upstream_changes",
                "merge_apply",
                "merge_preview",
                "migration_dry_run",
                "recall_context",
                "search_entities",
                "set_relation_status",
                "update_entity",
            ]
            .map(str::to_owned)
            .to_vec()
        );
    }
}
