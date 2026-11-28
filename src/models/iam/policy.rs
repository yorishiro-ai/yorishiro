//! The IAM policy document: its actions, resources and effects, parsing with size limits, and the evaluation of whether it allows a request.
//!
//! A document is `{"Version": "2012-10-17", "Statement": [...]}`.
//! A statement holds an optional `Sid`, an `Effect` (`Allow` or `Deny`), exactly one of `Action` and `NotAction`, and a `Resource`.
//! `Action`, `NotAction` and `Resource` take one string or a list of strings.
//! An action selector is `*`, `<service>:*`, or `<service>:<Name>` where `Name` may contain the wildcards `*` and `?`.
//! A resource selector is `*` or a YRN: `yrn:tenant:<tenant-id>`, `yrn:workspace:<tenant-id>:<workspace-id>` or `yrn:workspace:<tenant-id>:*`.

use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::YorishiroError;

/// The only accepted value of a document's `Version`.
pub(crate) const VERSION: &str = "2012-10-17";
const YRN_PREFIX: &str = "yrn:";
const MAX_STATEMENTS: usize = 64;
pub(crate) const MAX_ACTIONS: usize = 32;
const MAX_RESOURCES: usize = 32;
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const MAX_SID_BYTES: usize = 128;

/// The part of the product an action belongs to; it is the prefix of a qualified action name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    Iam,
    Audit,
    Tenant,
    Workspace,
    Schema,
    Entity,
    Migration,
    Inference,
    Template,
    Marketplace,
}

impl Service {
    pub const ALL: &'static [Self] = &[
        Self::Iam,
        Self::Audit,
        Self::Tenant,
        Self::Workspace,
        Self::Schema,
        Self::Entity,
        Self::Migration,
        Self::Inference,
        Self::Template,
        Self::Marketplace,
    ];

    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Iam => "iam",
            Self::Audit => "audit",
            Self::Tenant => "tenant",
            Self::Workspace => "workspace",
            Self::Schema => "schema",
            Self::Entity => "entity",
            Self::Migration => "migration",
            Self::Inference => "inference",
            Self::Template => "template",
            Self::Marketplace => "marketplace",
        }
    }

    fn from_prefix(prefix: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|service| service.prefix().eq_ignore_ascii_case(prefix))
    }
}

/// What an action does to the state it touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    List,
    Read,
    Write,
    PermissionsManagement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Tenant,
    Workspace,
}

macro_rules! actions {
    ($($variant:ident: $service:ident, $level:ident, $scope:ident;)*) => {
        /// Every operation a policy can allow or deny.
        /// The tool name, the REST operation id and the qualified policy name all derive from the variant name.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Action {
            $($variant),*
        }

        impl Action {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant)),*
                }
            }

            #[must_use]
            pub const fn service(self) -> Service {
                match self {
                    $(Self::$variant => Service::$service),*
                }
            }

            #[must_use]
            pub const fn access_level(self) -> AccessLevel {
                match self {
                    $(Self::$variant => AccessLevel::$level),*
                }
            }

            const fn scope(self) -> Scope {
                match self {
                    $(Self::$variant => Scope::$scope),*
                }
            }
        }
    };
}

actions! {
    ListUsers: Iam, List, Tenant;
    GetUser: Iam, Read, Tenant;
    CreateUser: Iam, Write, Tenant;
    UpdateUser: Iam, Write, Tenant;
    DeleteUser: Iam, Write, Tenant;
    UpdateLoginProfile: Iam, Write, Tenant;
    ListGroups: Iam, List, Tenant;
    GetGroup: Iam, Read, Tenant;
    CreateGroup: Iam, Write, Tenant;
    UpdateGroup: Iam, Write, Tenant;
    DeleteGroup: Iam, Write, Tenant;
    ListGroupsForUser: Iam, List, Tenant;
    AddUserToGroup: Iam, PermissionsManagement, Tenant;
    RemoveUserFromGroup: Iam, PermissionsManagement, Tenant;
    ListPolicies: Iam, List, Tenant;
    GetPolicy: Iam, Read, Tenant;
    CreatePolicy: Iam, PermissionsManagement, Tenant;
    DeletePolicy: Iam, PermissionsManagement, Tenant;
    CreatePolicyVersion: Iam, PermissionsManagement, Tenant;
    GetPolicyVersion: Iam, Read, Tenant;
    ListPolicyVersions: Iam, List, Tenant;
    SetDefaultPolicyVersion: Iam, PermissionsManagement, Tenant;
    DeletePolicyVersion: Iam, PermissionsManagement, Tenant;
    AttachUserPolicy: Iam, PermissionsManagement, Tenant;
    DetachUserPolicy: Iam, PermissionsManagement, Tenant;
    ListAttachedUserPolicies: Iam, List, Tenant;
    AttachGroupPolicy: Iam, PermissionsManagement, Tenant;
    DetachGroupPolicy: Iam, PermissionsManagement, Tenant;
    ListAttachedGroupPolicies: Iam, List, Tenant;
    ListAccessKeys: Iam, List, Tenant;
    CreateAccessKey: Iam, Write, Tenant;
    UpdateAccessKey: Iam, Write, Tenant;
    DeleteAccessKey: Iam, Write, Tenant;
    SimulatePrincipalPolicy: Iam, Read, Tenant;
    ListApiEvents: Audit, List, Tenant;
    GetTenant: Tenant, Read, Tenant;
    UpdateTenant: Tenant, Write, Tenant;
    DisableTenant: Tenant, Write, Tenant;
    GetTenantRetirementInventory: Tenant, Read, Tenant;
    ListWorkspaces: Workspace, List, Tenant;
    DiscoverWorkspaces: Workspace, List, Tenant;
    CreateWorkspace: Workspace, Write, Tenant;
    GetWorkspace: Workspace, Read, Workspace;
    UpdateWorkspace: Workspace, Write, Workspace;
    DeleteWorkspace: Workspace, Write, Workspace;
    ReindexWorkspace: Workspace, Write, Workspace;
    ExportWorkspace: Workspace, Read, Workspace;
    CreateImportJob: Workspace, Write, Workspace;
    GetImportJob: Workspace, Read, Workspace;
    GetWorkspaceEmbeddingProvider: Workspace, Read, Workspace;
    UpdateWorkspaceEmbeddingProvider: Workspace, Write, Workspace;
    DeleteWorkspaceEmbeddingProvider: Workspace, Write, Workspace;
    GetWorkspaceLlmProvider: Workspace, Read, Workspace;
    UpdateWorkspaceLlmProvider: Workspace, Write, Workspace;
    DeleteWorkspaceLlmProvider: Workspace, Write, Workspace;
    GetWorkspaceWorkerClass: Workspace, Read, Workspace;
    UpdateWorkspaceWorkerClass: Workspace, Write, Workspace;
    DeleteWorkspaceWorkerClass: Workspace, Write, Workspace;
    ListWorkspaceEntityColumns: Workspace, List, Workspace;
    UpdateWorkspaceEntityColumns: Workspace, Write, Workspace;
    DeleteWorkspaceEntityColumns: Workspace, Write, Workspace;
    ListSchemas: Schema, List, Workspace;
    GetSchema: Schema, Read, Workspace;
    GetActiveSchema: Schema, Read, Workspace;
    GetEntityTypeJsonSchema: Schema, Read, Workspace;
    CreateSchema: Schema, Write, Workspace;
    ArchiveSchema: Schema, Write, Workspace;
    DeleteSchema: Schema, Write, Workspace;
    ListSchemaForks: Schema, List, Workspace;
    GetSchemaFork: Schema, Read, Workspace;
    CreateSchemaFork: Schema, Write, Workspace;
    UpdateSchemaFork: Schema, Write, Workspace;
    DeleteSchemaFork: Schema, Write, Workspace;
    ListSchemaUpstreamChanges: Schema, List, Workspace;
    PreviewSchemaMerge: Schema, Read, Workspace;
    ApplySchemaMerge: Schema, Write, Workspace;
    ListEntities: Entity, List, Workspace;
    GetEntity: Entity, Read, Workspace;
    CreateEntity: Entity, Write, Workspace;
    UpdateEntity: Entity, Write, Workspace;
    DeleteEntity: Entity, Write, Workspace;
    GetEntityDrift: Entity, Read, Workspace;
    ListRelations: Entity, List, Workspace;
    GetRelation: Entity, Read, Workspace;
    CreateRelation: Entity, Write, Workspace;
    UpdateRelation: Entity, Write, Workspace;
    DeleteRelation: Entity, Write, Workspace;
    SearchEntities: Entity, Read, Workspace;
    RecallEntityContext: Entity, Read, Workspace;
    PreviewMigration: Migration, Read, Workspace;
    FillEntityDefaults: Migration, Write, Workspace;
    GetMigrationJob: Migration, Read, Workspace;
    UndoMigrationJob: Migration, Write, Workspace;
    CreateInferenceJob: Inference, Write, Workspace;
    GetInferenceJob: Inference, Read, Workspace;
    ListInferenceProposals: Inference, List, Workspace;
    ConfirmInferenceProposals: Inference, Write, Workspace;
    DiscardInferenceProposals: Inference, Write, Workspace;
    GetInferenceConfirmation: Inference, Read, Workspace;
    ListTemplateLibraryItems: Template, List, Tenant;
    GetTemplateLibraryItem: Template, Read, Tenant;
    CreateTemplateLibraryItem: Template, Write, Tenant;
    UpdateTemplateLibraryItem: Template, Write, Tenant;
    DeleteTemplateLibraryItem: Template, Write, Tenant;
    ForkTemplateLibraryItem: Template, Write, Tenant;
    ListBuiltinTemplates: Template, List, Tenant;
    GetBuiltinTemplate: Template, Read, Tenant;
    ListMarketplaceTemplates: Marketplace, List, Tenant;
    GetMarketplaceTemplate: Marketplace, Read, Tenant;
    ForkMarketplaceTemplate: Marketplace, Write, Tenant;
    ListMarketplaceVersions: Marketplace, List, Tenant;
    GetMarketplaceVersion: Marketplace, Read, Tenant;
    PublishMarketplaceVersion: Marketplace, Write, Tenant;
    UpdateMarketplaceVisibility: Marketplace, Write, Tenant;
    ListMarketplaceReviews: Marketplace, List, Tenant;
    CreateMarketplaceReview: Marketplace, Write, Tenant;
}

impl Action {
    /// The name a policy document uses, such as `iam:CreateGroup`.
    #[must_use]
    pub fn qualified(self) -> String {
        format!("{}:{}", self.service().prefix(), self.name())
    }

    /// Looks an action up by its bare name, such as `CreateGroup`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.name() == name)
    }

    /// Looks an action up by its qualified name, ignoring case.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|action| action.qualified().eq_ignore_ascii_case(text))
    }

    pub(crate) const fn applies_to(self, resource: Resource) -> bool {
        match self.scope() {
            Scope::Tenant => matches!(resource, Resource::Tenant { .. }),
            Scope::Workspace => matches!(
                resource,
                Resource::Workspace { .. } | Resource::AllWorkspaces { .. }
            ),
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.qualified())
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom("unknown action"))
    }
}

/// Matches `text` against a pattern whose `*` stands for any run of characters and `?` for one, ignoring case.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let (mut p, mut t, mut star, mut mark) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Expands one action selector into the actions it names.
fn select_actions(selector: &str) -> Result<Vec<Action>, YorishiroError> {
    if selector == "*" {
        return Ok(Action::ALL.to_vec());
    }
    let (service, name) = selector
        .split_once(':')
        .ok_or_else(|| invalid("action must be * or <service>:<Name>"))?;
    let service =
        Service::from_prefix(service).ok_or_else(|| invalid("action names an unknown service"))?;
    let matched: Vec<Action> = Action::ALL
        .iter()
        .copied()
        .filter(|action| action.service() == service && glob(name, action.name()))
        .collect();
    if matched.is_empty() {
        return Err(invalid("action matches no known action"));
    }
    Ok(matched)
}

/// What a request acts on, built by the caller from the credential and the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Tenant { tenant_id: Uuid },
    Workspace { tenant_id: Uuid, workspace_id: Uuid },
    AllWorkspaces { tenant_id: Uuid },
}

impl Resource {
    pub(crate) const fn tenant_id(self) -> Uuid {
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

    /// The YRN a document uses for this resource.
    #[must_use]
    pub fn yrn(self) -> String {
        match self {
            Self::Tenant { tenant_id } => format!("{YRN_PREFIX}tenant:{tenant_id}"),
            Self::Workspace {
                tenant_id,
                workspace_id,
            } => format!("{YRN_PREFIX}workspace:{tenant_id}:{workspace_id}"),
            Self::AllWorkspaces { tenant_id } => format!("{YRN_PREFIX}workspace:{tenant_id}:*"),
        }
    }

    /// Parses a resource YRN.
    ///
    /// # Errors
    /// Returns a validation error when `yrn` is not a tenant or workspace YRN.
    pub(crate) fn parse(yrn: &str) -> Result<Self, YorishiroError> {
        let rest = yrn
            .strip_prefix(YRN_PREFIX)
            .ok_or_else(|| invalid("resource must be * or a yrn: YRN"))?;
        let fields: Vec<&str> = rest.split(':').collect();
        let tenant = |field: &str| {
            Uuid::parse_str(field).map_err(|_| invalid("resource YRN needs a tenant id"))
        };
        match fields.as_slice() {
            ["tenant", tenant_id] => Ok(Self::Tenant {
                tenant_id: tenant(tenant_id)?,
            }),
            ["workspace", tenant_id, "*"] => Ok(Self::AllWorkspaces {
                tenant_id: tenant(tenant_id)?,
            }),
            ["workspace", tenant_id, workspace_id] => Ok(Self::Workspace {
                tenant_id: tenant(tenant_id)?,
                workspace_id: Uuid::parse_str(workspace_id)
                    .map_err(|_| invalid("workspace YRN needs a workspace id"))?,
            }),
            _ => Err(invalid("resource YRN names an unsupported resource")),
        }
    }
}

#[derive(Debug)]
enum ResourceSelector {
    Any,
    Exact(Resource),
}

impl ResourceSelector {
    fn matches(&self, target: Resource) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(resource) => resource.matches(target),
        }
    }

    const fn applies_to(&self, action: Action) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(resource) => action.applies_to(*resource),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
pub(crate) enum Effect {
    Allow,
    Deny,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(one) => vec![one],
            Self::Many(many) => many,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "IamPolicyStatement", extend("oneOf" = [
    {"required": ["Action"], "not": {"required": ["NotAction"]}},
    {"required": ["NotAction"], "not": {"required": ["Action"]}}
]))]
struct RawStatement {
    #[serde(rename = "Sid")]
    sid: Option<String>,
    #[serde(rename = "Effect")]
    effect: Effect,
    #[serde(rename = "Action")]
    action: Option<OneOrMany<String>>,
    #[serde(rename = "NotAction")]
    not_action: Option<OneOrMany<String>>,
    #[serde(rename = "Resource")]
    resource: OneOrMany<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "IamPolicyDocument", extend("x-iam-actions" = action_reference()))]
pub(crate) struct RawDocument {
    #[serde(rename = "Version")]
    #[schemars(extend("const" = VERSION))]
    version: String,
    #[serde(rename = "Statement")]
    statement: OneOrMany<RawStatement>,
}

fn action_reference() -> std::collections::BTreeMap<String, AccessLevel> {
    Action::ALL
        .iter()
        .map(|action| (action.qualified(), action.access_level()))
        .collect()
}

#[derive(Debug)]
struct Statement {
    effect: Effect,
    actions: HashSet<Action>,
    resources: Vec<ResourceSelector>,
}

#[derive(Debug)]
pub struct Policy {
    tenant_id: Uuid,
    statements: Vec<Statement>,
}

fn invalid(message: &'static str) -> YorishiroError {
    YorishiroError::ValidationFailed {
        message: message.into(),
        details: vec![],
        hint: "Provide a valid policy document for this tenant".into(),
    }
}

fn raw_document(document: &str) -> Result<RawDocument, YorishiroError> {
    if document.len() > MAX_DOCUMENT_BYTES {
        return Err(invalid("policy document is too large"));
    }
    let raw: RawDocument =
        serde_json::from_str(document).map_err(|_| invalid("invalid policy document"))?;
    if raw.version != VERSION {
        return Err(invalid("policy document Version must be 2012-10-17"));
    }
    Ok(raw)
}

impl Policy {
    /// The workspaces a document names by YRN, without validating the rest of it.
    pub(crate) fn workspace_ids(document: &str) -> Result<Vec<Uuid>, YorishiroError> {
        Ok(raw_document(document)?
            .statement
            .into_vec()
            .into_iter()
            .flat_map(|statement| statement.resource.into_vec())
            .filter_map(|yrn| match Resource::parse(&yrn) {
                Ok(Resource::Workspace { workspace_id, .. }) => Some(workspace_id),
                _ => None,
            })
            .collect())
    }

    /// # Errors
    /// Returns a validation error for malformed, unsupported, oversized, or cross-tenant documents.
    pub fn parse(
        document: &str,
        tenant_id: Uuid,
        owns_workspace: impl Fn(Uuid, Uuid) -> bool,
    ) -> Result<Self, YorishiroError> {
        Self::parse_with(document, tenant_id, owns_workspace, true)
    }

    /// Parses a document the product itself generated, which is exempt from the per-statement list limits.
    pub(crate) fn parse_generated(document: &str, tenant_id: Uuid) -> Result<Self, YorishiroError> {
        Self::parse_with(document, tenant_id, |_, _| true, false)
    }

    fn parse_with(
        document: &str,
        tenant_id: Uuid,
        owns_workspace: impl Fn(Uuid, Uuid) -> bool,
        enforce_limits: bool,
    ) -> Result<Self, YorishiroError> {
        let raw = raw_document(document)?;
        let raw_statements = raw.statement.into_vec();
        if raw_statements.is_empty() || raw_statements.len() > MAX_STATEMENTS {
            return Err(invalid("invalid statement count"));
        }
        let mut statements = Vec::with_capacity(raw_statements.len());
        for (index, raw) in raw_statements.into_iter().enumerate() {
            let statement = Self::parse_statement(raw, tenant_id, &owns_workspace, enforce_limits)
                .map_err(|mut error| {
                    if let YorishiroError::ValidationFailed { message, .. } = &mut error {
                        *message = format!("Statement[{index}]: {message}");
                    }
                    error
                })?;
            statements.push(statement);
        }
        Ok(Self {
            tenant_id,
            statements,
        })
    }

    fn parse_statement(
        raw: RawStatement,
        tenant_id: Uuid,
        owns_workspace: impl Fn(Uuid, Uuid) -> bool,
        enforce_limits: bool,
    ) -> Result<Statement, YorishiroError> {
        if raw.sid.as_ref().is_some_and(|sid| {
            sid.len() > MAX_SID_BYTES || !sid.chars().all(|c| c.is_ascii_alphanumeric())
        }) {
            return Err(invalid("Sid must be alphanumeric"));
        }
        let resources = raw.resource.into_vec();
        if resources.is_empty() || (enforce_limits && resources.len() > MAX_RESOURCES) {
            return Err(invalid("invalid resource count"));
        }
        let mut selectors = Vec::with_capacity(resources.len());
        for resource in &resources {
            if resource == "*" {
                selectors.push(ResourceSelector::Any);
                continue;
            }
            let parsed = Resource::parse(resource)?;
            if parsed.tenant_id() != tenant_id {
                return Err(invalid("resource belongs to another tenant"));
            }
            if let Resource::Workspace {
                tenant_id,
                workspace_id,
            } = parsed
                && !owns_workspace(tenant_id, workspace_id)
            {
                return Err(invalid("workspace ownership could not be verified"));
            }
            selectors.push(ResourceSelector::Exact(parsed));
        }
        let actions = match (raw.action, raw.not_action) {
            (Some(action), None) => {
                let selectors_text = action.into_vec();
                if selectors_text.is_empty()
                    || (enforce_limits && selectors_text.len() > MAX_ACTIONS)
                {
                    return Err(invalid("invalid action count"));
                }
                let mut actions = HashSet::new();
                for selector in &selectors_text {
                    let matched = select_actions(selector)?;
                    if selector != "*"
                        && !matched
                            .iter()
                            .any(|action| selectors.iter().any(|r| r.applies_to(*action)))
                    {
                        return Err(invalid("action and resource scopes do not match"));
                    }
                    actions.extend(matched);
                }
                actions
            }
            (None, Some(not_action)) => {
                let selectors_text = not_action.into_vec();
                if selectors_text.is_empty()
                    || (enforce_limits && selectors_text.len() > MAX_ACTIONS)
                {
                    return Err(invalid("invalid action count"));
                }
                let mut excluded = HashSet::new();
                for selector in &selectors_text {
                    excluded.extend(select_actions(selector)?);
                }
                Action::ALL
                    .iter()
                    .copied()
                    .filter(|action| !excluded.contains(action))
                    .collect()
            }
            _ => {
                return Err(invalid(
                    "a statement needs exactly one of Action and NotAction",
                ));
            }
        };
        if selectors
            .iter()
            .any(|selector| !actions.iter().any(|action| selector.applies_to(*action)))
        {
            return Err(invalid("action and resource scopes do not match"));
        }
        Ok(Statement {
            effect: raw.effect,
            actions,
            resources: selectors,
        })
    }

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
        Self::decide(policies, action, target).decision == Decision::Allowed
    }

    /// Evaluates a hypothetical request and reports which policies decided it.
    /// Unlike a real request, the target may be every workspace of the tenant.
    #[must_use]
    pub fn simulate(
        policies: &[Self],
        tenant_id: Uuid,
        action: Action,
        target: Resource,
    ) -> Evaluation {
        if target.tenant_id() != tenant_id
            || !action.applies_to(target)
            || policies.iter().any(|policy| policy.tenant_id != tenant_id)
        {
            return Evaluation {
                decision: Decision::ImplicitDeny,
                matched: Vec::new(),
            };
        }
        Self::decide(policies, action, target)
    }

    /// Deny beats allow; without a matching statement the request is denied implicitly.
    fn decide(policies: &[Self], action: Action, target: Resource) -> Evaluation {
        let mut allowing = Vec::new();
        let mut denying = Vec::new();
        for (index, policy) in policies.iter().enumerate() {
            for statement in &policy.statements {
                if statement.actions.contains(&action)
                    && statement.resources.iter().any(|r| r.matches(target))
                {
                    match statement.effect {
                        Effect::Allow => allowing.push(index),
                        Effect::Deny => denying.push(index),
                    }
                }
            }
        }
        let (decision, mut matched) = if !denying.is_empty() {
            (Decision::ExplicitDeny, denying)
        } else if !allowing.is_empty() {
            (Decision::Allowed, allowing)
        } else {
            (Decision::ImplicitDeny, Vec::new())
        };
        matched.sort_unstable();
        matched.dedup();
        Evaluation { decision, matched }
    }
}

/// The outcome of evaluating a request against a set of policies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allowed,
    ImplicitDeny,
    ExplicitDeny,
}

/// A decision and the positions, in the evaluated slice, of the policies whose statements matched it.
#[derive(Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub decision: Decision,
    pub matched: Vec<usize>,
}
