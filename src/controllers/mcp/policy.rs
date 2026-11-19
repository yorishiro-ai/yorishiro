//! Which tools an MCP server exposes and which of them a request may see or call.

use super::*;

pub trait McpToolPolicy: Send + Sync {
    fn allows(&self, ctx: &AppContext, name: &str) -> bool;
    fn read_only(&self, name: &str) -> bool {
        read_only_tool(name)
    }
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
        "capabilities"
            | "whoami"
            | "discover_workspaces"
            | "export_jsonl"
            | "get_active_schema"
            | "get_current_user"
            | "get_entity"
            | "get_entity_drift"
            | "get_entity_type_json_schema"
            | "get_iam_group"
            | "get_iam_policy"
            | "get_relation"
            | "get_schema_by_id"
            | "get_template"
            | "get_template_library_item"
            | "get_tenant"
            | "get_workspace"
            | "import_job_status"
            | "migration_job_status"
            | "migration_dry_run"
            | "list_api_keys"
            | "list_audit_log"
            | "list_entities"
            | "list_iam_audit"
            | "list_iam_group_members"
            | "list_iam_group_policies"
            | "list_iam_groups"
            | "list_iam_policies"
            | "list_iam_roles"
            | "get_iam_role"
            | "list_iam_role_policies"
            | "list_iam_user_policies"
            | "list_members"
            | "list_relations"
            | "list_schemas"
            | "list_template_library"
            | "list_templates"
            | "list_workspace_members"
            | "list_workspaces"
            | "preview_member_removal"
            | "recall_context"
            | "search_entities"
            | "tenant_retirement_inventory"
    )
}
