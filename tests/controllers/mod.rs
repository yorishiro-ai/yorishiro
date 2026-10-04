mod mcp;
mod route_inventory;
#[cfg(feature = "openapi")]
mod swagger;

use rmcp::model::Tool;

/// One `## <title>` section of the MCP tool inventory document, as `docs/en/mcp-tools.md` lists it.
pub(crate) fn render_inventory_section(title: &str, tools: &[Tool]) -> String {
    let mut output = String::new();
    output.push_str("## ");
    output.push_str(title);
    output.push_str("\n\n");
    for tool in tools {
        output.push_str("### `");
        output.push_str(&tool.name);
        output.push_str("`\n\n");
        output.push_str(tool.description.as_deref().unwrap_or(""));
        output.push_str("\n\nInput schema:\n\n```json\n");
        output.push_str(
            &serde_json::to_string_pretty(&tool.schema_as_json_value())
                .expect("MCP tool schema is serializable"),
        );
        output.push_str("\n```\n\n");
    }
    output
}
