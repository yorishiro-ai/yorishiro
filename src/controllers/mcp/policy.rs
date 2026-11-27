//! Which tools an MCP server exposes and which of them a request may see or call.

use super::{
    AppContext, Arc, ToolRouter, YorishiroMcpServer, community_tool_router, compose_tool_routers,
};

pub trait McpToolPolicy: Send + Sync {
    fn allows(&self, ctx: &AppContext, name: &str) -> bool;
    fn accepts_workspace_selection(&self, name: &str) -> bool {
        workspace_selection_tool(name)
    }
    fn read_only(&self, name: &str) -> bool {
        read_only_tool(name)
    }
}

pub(crate) fn workspace_selection_tool(name: &str) -> bool {
    matches!(
        name,
        "archive_schema"
            | "create_access_key"
            | "create_entity"
            | "create_relation"
            | "create_schema"
            | "delete_entity"
            | "delete_relation"
            | "delete_schema"
            | "export_workspace"
            | "fill_entity_defaults"
            | "get_active_schema"
            | "get_entity"
            | "get_entity_drift"
            | "get_entity_type_json_schema"
            | "get_group"
            | "get_policy"
            | "get_policy_version"
            | "get_relation"
            | "get_schema"
            | "get_import_job"
            | "create_import_job"
            | "list_workspace_audit_events"
            | "list_attached_group_policies"
            | "list_attached_user_policies"
            | "list_entities"
            | "list_groups"
            | "list_groups_for_user"
            | "list_iam_audit_events"
            | "list_api_events"
            | "list_policies"
            | "list_policy_versions"
            | "list_relations"
            | "list_schemas"
            | "preview_migration"
            | "get_migration_job"
            | "recall_entity_context"
            | "reindex_workspace"
            | "search_entities"
            | "update_relation"
            | "undo_migration_job"
            | "update_entity"
            | "get_caller_identity"
    )
}

/// The policy a deployment gets when it does not choose one: every registered tool is listed and callable.
pub(crate) struct AllowAllTools;

impl McpToolPolicy for AllowAllTools {
    fn allows(&self, _ctx: &AppContext, _name: &str) -> bool {
        true
    }
}

/// The tools one MCP server exposes, and the policy that decides which of them a request may see.
///
/// Starts as the community tool set under `AllowAllTools`.
/// An edition adds its own tools with [`McpToolSet::extend`], which replaces the policy because only the edition knows which of the tools it added need gating.
#[derive(Clone)]
pub struct McpToolSet {
    router: ToolRouter<YorishiroMcpServer>,
    policy: Arc<dyn McpToolPolicy>,
}

impl McpToolSet {
    pub(crate) fn community() -> Self {
        Self {
            router: community_tool_router(),
            policy: Arc::new(AllowAllTools),
        }
    }

    /// Adds `router`'s tools and decides access with `policy` from now on.
    ///
    /// # Panics
    /// Panics if `router` names a tool this set already has.
    #[must_use]
    pub fn extend(
        self,
        router: ToolRouter<YorishiroMcpServer>,
        policy: Arc<dyn McpToolPolicy>,
    ) -> Self {
        Self {
            router: compose_tool_routers([self.router, router]),
            policy,
        }
    }

    pub(crate) fn server(&self, ctx: AppContext) -> YorishiroMcpServer {
        YorishiroMcpServer::new(ctx, self.router.clone(), self.policy.clone())
    }
}

// Unknown tools fail closed in read-only mode; scope alone cannot distinguish reads from IAM writes.
pub(crate) fn read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "get_caller_identity"
            | "discover_workspaces"
            | "export_workspace"
            | "get_active_schema"
            | "get_entity"
            | "get_entity_drift"
            | "get_entity_type_json_schema"
            | "get_group"
            | "get_policy"
            | "get_policy_version"
            | "get_relation"
            | "get_schema"
            | "get_builtin_template"
            | "get_template_library_item"
            | "get_tenant"
            | "get_user"
            | "get_workspace"
            | "get_import_job"
            | "get_migration_job"
            | "preview_migration"
            | "list_access_keys"
            | "list_workspace_audit_events"
            | "list_attached_group_policies"
            | "list_attached_user_policies"
            | "list_entities"
            | "list_groups"
            | "list_groups_for_user"
            | "list_iam_audit_events"
            | "list_api_events"
            | "list_policies"
            | "list_policy_versions"
            | "list_relations"
            | "list_schemas"
            | "list_template_library_items"
            | "list_builtin_templates"
            | "list_users"
            | "list_workspaces"
            | "recall_entity_context"
            | "search_entities"
            | "simulate_principal_policy"
            | "get_tenant_retirement_inventory"
    )
}
