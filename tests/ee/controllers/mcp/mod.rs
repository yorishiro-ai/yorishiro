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
use yorishiro::models::iam::policy::Action;

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

#[test]
fn tool_names_match_the_policy_action_catalog() {
    let names: HashSet<_> =
        yorishiro::controllers::mcp::compose_tool_routers([community_tool_router(), tool_router()])
            .list_all()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect();
    let mut expected = HashSet::new();
    for action in Action::ALL {
        let mut tool = String::new();
        for (index, ch) in action.name().chars().enumerate() {
            if index != 0 && ch.is_ascii_uppercase() {
                tool.push('_');
            }
            tool.push(ch.to_ascii_lowercase());
        }
        assert!(expected.insert(tool), "duplicate tool name for {action:?}");
    }
    // Caller identity is available without a policy grant.
    expected.insert("get_caller_identity".to_owned());
    // Legacy operations retain their current authorization until their removal.
    expected.insert("reject_inference_proposals".to_owned());
    assert_eq!(names, expected);
}
