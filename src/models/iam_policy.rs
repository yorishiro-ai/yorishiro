use serde::Deserialize;
use uuid::Uuid;

use crate::YorishiroError;

const MAX_STATEMENTS: usize = 64;
const MAX_ACTIONS: usize = 32;
const MAX_RESOURCES: usize = 32;
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Supported policy actions corresponding to existing application operations.
pub enum Action {
    TenantGet,
    TenantUpdate,
    WorkspaceList,
    WorkspaceCreate,
    WorkspaceGet,
    WorkspaceUpdate,
    WorkspaceDelete,
    EntityList,
    EntityGet,
    EntityCreate,
    EntityUpdate,
    EntityDelete,
    EntityDriftGet,
    RelationList,
    RelationGet,
    RelationCreate,
    RelationUpdate,
    RelationDelete,
    SchemaList,
    SchemaGet,
    SchemaCreate,
    SchemaArchive,
    SchemaDelete,
    SchemaForkList,
    SchemaForkGet,
    SchemaForkCreate,
    SchemaForkUpdate,
    SchemaForkDelete,
    SchemaUpstreamList,
    SchemaMergePreview,
    SchemaMergeApply,
    Search,
    Recall,
    Export,
    ImportCreate,
    ImportJobGet,
    MigrationDryRun,
    MigrationJobGet,
    MigrationUndo,
    MigrationFillDefaults,
    WorkspaceReindex,
    InferenceCreate,
    InferenceJobGet,
    InferenceProposalList,
    InferenceConfirm,
    InferenceReject,
    InferenceDiscard,
    InferenceConfirmationGet,
    EmbeddingProviderGet,
    EmbeddingProviderUpdate,
    EmbeddingProviderDelete,
    LlmProviderGet,
    LlmProviderUpdate,
    LlmProviderDelete,
    WorkerClassGet,
    WorkerClassUpdate,
    WorkerClassDelete,
    EntityColumnsGet,
    EntityColumnsUpdate,
    EntityColumnsDelete,
    AuditLogList,
    IamAuditList,
    CredentialList,
    CredentialCreate,
    CredentialRotate,
    CredentialRevoke,
    TenantUserList,
    TenantUserCreate,
    TenantUserUpdate,
    TenantUserDelete,
    TenantUserPasswordReset,
    GroupList,
    GroupGet,
    GroupCreate,
    GroupUpdate,
    GroupDelete,
    GroupMemberList,
    GroupMemberAdd,
    GroupMemberRemove,
    PolicyList,
    PolicyGet,
    PolicyCreate,
    PolicyUpdate,
    PolicyDelete,
    UserPolicyList,
    GroupPolicyList,
    UserPolicyAttach,
    UserPolicyDetach,
    GroupPolicyAttach,
    GroupPolicyDetach,
    TemplateLibraryList,
    TemplateLibraryGet,
    TemplateLibraryCreate,
    TemplateLibraryUpdate,
    TemplateLibraryDelete,
    TemplateLibraryFork,
    TenantOverviewGet,
    BuiltinTemplateList,
    BuiltinTemplateGet,
    MarketplaceList,
    MarketplaceVersionList,
    MarketplacePublish,
    MarketplaceReviewList,
    MarketplaceReviewSubmit,
    MarketplaceFork,
    MarketplaceVisibilityUpdate,
}

impl Action {
    pub(crate) fn applies_to(self, resource: Resource) -> bool {
        match self {
            Self::TenantGet
            | Self::IamAuditList
            | Self::TenantUpdate
            | Self::WorkspaceList
            | Self::WorkspaceCreate
            | Self::CredentialList
            | Self::CredentialCreate
            | Self::CredentialRotate
            | Self::CredentialRevoke
            | Self::TenantUserList
            | Self::TenantUserCreate
            | Self::TenantUserUpdate
            | Self::TenantUserDelete
            | Self::TenantUserPasswordReset
            | Self::GroupList
            | Self::GroupGet
            | Self::GroupCreate
            | Self::GroupUpdate
            | Self::GroupDelete
            | Self::GroupMemberList
            | Self::GroupMemberAdd
            | Self::GroupMemberRemove
            | Self::PolicyList
            | Self::PolicyGet
            | Self::PolicyCreate
            | Self::PolicyUpdate
            | Self::PolicyDelete
            | Self::UserPolicyList
            | Self::GroupPolicyList
            | Self::UserPolicyAttach
            | Self::UserPolicyDetach
            | Self::GroupPolicyAttach
            | Self::GroupPolicyDetach
            | Self::TemplateLibraryList
            | Self::TemplateLibraryGet
            | Self::TemplateLibraryCreate
            | Self::TemplateLibraryUpdate
            | Self::TemplateLibraryDelete
            | Self::TemplateLibraryFork
            | Self::TenantOverviewGet
            | Self::BuiltinTemplateList
            | Self::BuiltinTemplateGet
            | Self::MarketplaceList
            | Self::MarketplaceVersionList
            | Self::MarketplacePublish
            | Self::MarketplaceReviewList
            | Self::MarketplaceReviewSubmit
            | Self::MarketplaceFork
            | Self::MarketplaceVisibilityUpdate => {
                matches!(resource, Resource::Tenant { .. })
            }
            Self::WorkspaceGet
            | Self::WorkspaceUpdate
            | Self::WorkspaceDelete
            | Self::EntityList
            | Self::EntityGet
            | Self::EntityCreate
            | Self::EntityUpdate
            | Self::EntityDelete
            | Self::EntityDriftGet
            | Self::RelationList
            | Self::RelationGet
            | Self::RelationCreate
            | Self::RelationUpdate
            | Self::RelationDelete
            | Self::SchemaList
            | Self::SchemaGet
            | Self::SchemaCreate
            | Self::SchemaArchive
            | Self::SchemaDelete
            | Self::SchemaForkList
            | Self::SchemaForkGet
            | Self::SchemaForkCreate
            | Self::SchemaForkUpdate
            | Self::SchemaForkDelete
            | Self::SchemaUpstreamList
            | Self::SchemaMergePreview
            | Self::SchemaMergeApply
            | Self::Search
            | Self::Recall
            | Self::Export
            | Self::ImportCreate
            | Self::ImportJobGet
            | Self::MigrationDryRun
            | Self::MigrationJobGet
            | Self::MigrationUndo
            | Self::MigrationFillDefaults
            | Self::WorkspaceReindex
            | Self::InferenceCreate
            | Self::InferenceJobGet
            | Self::InferenceProposalList
            | Self::InferenceConfirm
            | Self::InferenceReject
            | Self::InferenceDiscard
            | Self::InferenceConfirmationGet
            | Self::EmbeddingProviderGet
            | Self::EmbeddingProviderUpdate
            | Self::EmbeddingProviderDelete
            | Self::LlmProviderGet
            | Self::LlmProviderUpdate
            | Self::LlmProviderDelete
            | Self::WorkerClassGet
            | Self::WorkerClassUpdate
            | Self::WorkerClassDelete
            | Self::EntityColumnsGet
            | Self::EntityColumnsUpdate
            | Self::EntityColumnsDelete
            | Self::AuditLogList => matches!(
                resource,
                Resource::Workspace { .. } | Resource::AllWorkspaces { .. }
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ActionSelector {
    All(All),
    Action(Action),
}

#[derive(Debug, Deserialize)]
enum All {
    #[serde(rename = "*")]
    All,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
/// A tenant-scoped resource or a selector for all workspaces in a tenant.
pub enum Resource {
    Tenant { tenant_id: Uuid },
    Workspace { tenant_id: Uuid, workspace_id: Uuid },
    AllWorkspaces { tenant_id: Uuid },
}

impl Resource {
    fn tenant_id(self) -> Uuid {
        match self {
            Self::Tenant { tenant_id }
            | Self::Workspace { tenant_id, .. }
            | Self::AllWorkspaces { tenant_id } => tenant_id,
        }
    }

    fn matches(self, target: Self) -> bool {
        self == target
            || matches!((self, target),
                (Self::AllWorkspaces { tenant_id: a }, Self::Workspace { tenant_id: b, .. }) if a == b)
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Effect {
    Allow,
    Deny,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Statement {
    effect: Effect,
    actions: Vec<ActionSelector>,
    resources: Vec<Resource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    statements: Vec<Statement>,
}

/// A validated policy bound to exactly one tenant.
#[derive(Debug)]
pub struct Policy {
    tenant_id: Uuid,
    statements: Vec<Statement>,
}

fn invalid(message: &'static str) -> YorishiroError {
    YorishiroError::ValidationFailed {
        message: message.into(),
        details: vec![],
        hint: "Provide a valid tenant-scoped policy document".into(),
    }
}

impl Policy {
    pub(crate) fn workspace_ids(document: &str) -> Result<Vec<Uuid>, YorishiroError> {
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("policy document is too large"));
        }
        let parsed: Document =
            serde_json::from_str(document).map_err(|_| invalid("invalid policy document"))?;
        Ok(parsed
            .statements
            .into_iter()
            .flat_map(|s| s.resources)
            .filter_map(|r| match r {
                Resource::Workspace { workspace_id, .. } => Some(workspace_id),
                _ => None,
            })
            .collect())
    }
    /// Parses and validates a document for the specified tenant.
    /// The verifier must use authoritative workspace ownership data and reject lookup failures.
    ///
    /// # Errors
    /// Returns a validation error for malformed, unsupported, oversized, or cross-tenant documents.
    pub fn parse(
        document: &str,
        tenant_id: Uuid,
        owns_workspace: impl Fn(Uuid, Uuid) -> bool,
    ) -> Result<Self, YorishiroError> {
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("policy document is too large"));
        }
        let policy: Document =
            serde_json::from_str(document).map_err(|_| invalid("invalid policy document"))?;
        if policy.statements.is_empty() || policy.statements.len() > MAX_STATEMENTS {
            return Err(invalid("invalid statement count"));
        }
        for statement in &policy.statements {
            if statement.actions.is_empty() || statement.actions.len() > MAX_ACTIONS {
                return Err(invalid("invalid action count"));
            }
            if statement.resources.is_empty() || statement.resources.len() > MAX_RESOURCES {
                return Err(invalid("invalid resource count"));
            }
            if statement
                .resources
                .iter()
                .any(|r| r.tenant_id() != tenant_id)
            {
                return Err(invalid("resource belongs to another tenant"));
            }
            if statement.resources.iter().any(|r| match *r {
                Resource::Workspace {
                    tenant_id,
                    workspace_id,
                } => !owns_workspace(tenant_id, workspace_id),
                _ => false,
            }) {
                return Err(invalid("workspace ownership could not be verified"));
            }
            if statement.actions.iter().any(|a| match a {
                ActionSelector::All(_) => false,
                ActionSelector::Action(action) => {
                    !statement.resources.iter().any(|r| action.applies_to(*r))
                }
            }) || statement.resources.iter().any(|r| {
                !statement.actions.iter().any(|a| match a {
                    ActionSelector::All(_) => true,
                    ActionSelector::Action(action) => action.applies_to(*r),
                })
            }) {
                return Err(invalid("action and resource scopes do not match"));
            }
        }
        Ok(Self {
            tenant_id,
            statements: policy.statements,
        })
    }

    /// Evaluates a collection with explicit deny precedence and default deny.
    ///
    /// The verifier must use current authoritative workspace ownership data and reject lookup failures.
    /// Concrete workspace selectors only match the same verified workspace target.
    /// Unrelated stale selectors do not affect the decision, and matching denies still take precedence.
    /// A collection containing a policy from any other tenant denies the request.
    pub fn allows(
        policies: &[Self],
        tenant_id: Uuid,
        action: Action,
        target: Resource,
        owns_workspace: impl Fn(Uuid, Uuid) -> bool,
    ) -> bool {
        if target.tenant_id() != tenant_id
            || matches!(target, Resource::AllWorkspaces { .. })
            || !action.applies_to(target)
            || policies.iter().any(|policy| policy.tenant_id != tenant_id)
            || matches!(target, Resource::Workspace { tenant_id, workspace_id } if !owns_workspace(tenant_id, workspace_id))
        {
            return false;
        }
        let mut allowed = false;
        for statement in policies.iter().flat_map(|p| &p.statements) {
            if statement.actions.iter().any(|a| match a {
                ActionSelector::All(_) => true,
                ActionSelector::Action(a) => *a == action,
            }) && statement.resources.iter().any(|r| r.matches(target))
            {
                if statement.effect == Effect::Deny {
                    return false;
                }
                allowed = true;
            }
        }
        allowed
    }
}
