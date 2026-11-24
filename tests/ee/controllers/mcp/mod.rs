mod dashboard;
mod embedding;
mod entity_columns;
mod llm;
mod origin;
mod schema_forks;
mod worker_class;
mod workspaces;

pub(crate) use crate::controllers::mcp::{
    initialize, mcp_call, provider_tools_keep_secrets_private_and_match_rest, rpc_body,
    tool_inventory, tool_result_json,
};

use std::collections::HashSet;

use yorishiro::controllers::mcp::community_tool_router;
use yorishiro::edition::ee::controllers::mcp::tool_router;

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
    crate::controllers::mcp::assert_inventory_contract("enterprise", &enterprise);
    assert!(community_names.is_disjoint(&enterprise_names));
    let expected_full_names: HashSet<_> =
        community_names.union(&enterprise_names).copied().collect();
    assert_eq!(full_names, expected_full_names);
}
