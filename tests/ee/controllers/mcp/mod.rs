use crate::controllers::render_inventory_section;
use std::collections::HashSet;

use yorishiro::controllers::mcp::community_tool_router;
use yorishiro::edition::ee::controllers::mcp::tool_router;

fn render_inventory_fragment(
    community: &[rmcp::model::Tool],
    enterprise: &[rmcp::model::Tool],
) -> String {
    let mut community = community.to_vec();
    let mut enterprise = enterprise.to_vec();
    community.sort_by(|a, b| a.name.cmp(&b.name));
    enterprise.sort_by(|a, b| a.name.cmp(&b.name));
    format!(
        "{}{}",
        render_inventory_section("Community tools", &community),
        render_inventory_section("Enterprise-only tools", &enterprise)
    )
}

#[test]
fn enterprise_inventory_contract_is_disjoint_and_contains_community() {
    let community = community_tool_router().list_all();
    let enterprise = tool_router().list_all();
    let full =
        yorishiro::controllers::mcp::compose_tool_routers([community_tool_router(), tool_router()])
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
        community_names.union(&enterprise_names).copied().collect();
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
    let docs = include_str!("../../../fixtures/mcp-inventory.md");
    let start = "<!-- BEGIN GENERATED MCP INVENTORY -->\n";
    let end = "<!-- END GENERATED MCP INVENTORY -->";
    let generated = docs
        .split_once(start)
        .and_then(|(_, rest)| rest.split_once(end).map(|(body, _)| body))
        .expect("English MCP documentation markers");
    let expected = render_inventory_fragment(&community, &enterprise);
    if std::env::var_os("UPDATE_FIXTURES").is_some() && generated != expected {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mcp-inventory.md"
        );
        std::fs::write(path, docs.replacen(generated, &expected, 1))
            .expect("rewrite the MCP inventory");
        return;
    }
    assert_eq!(
        generated, expected,
        "MCP inventory drifted; run `UPDATE_FIXTURES=1 cargo test --features enterprise enterprise_inventory_contract` and review the diff"
    );
}
