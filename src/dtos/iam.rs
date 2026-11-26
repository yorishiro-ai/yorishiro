//! Request and response types shared by the IAM endpoints: pagination, the audit log and policy simulation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListIam {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListAudit {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AuditRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub access_key_id: Uuid,
    /// The action name, such as `CreateGroup`.
    pub action: String,
    pub detail: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
}

impl From<crate::models::iam_audit_events::Model> for AuditRecord {
    fn from(event: crate::models::iam_audit_events::Model) -> Self {
        Self {
            id: event.id,
            tenant_id: event.tenant_id,
            user_id: event.user_id,
            access_key_id: event.access_key_id,
            action: event.action,
            detail: event.detail,
            created_at: event.created_at,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SimulatePrincipalPolicyRequest {
    /// The user whose policies are evaluated; defaults to the caller.
    pub user_id: Option<Uuid>,
    /// The qualified action name, such as `iam:CreateGroup` or `entity:GetEntity`.
    pub action: String,
    /// The tenant YRN, a workspace YRN, or `*`.
    /// Omitted or `*` means the tenant for a tenant action and every workspace for a workspace action.
    pub resource: Option<String>,
}

impl SimulatePrincipalPolicyRequest {
    pub(crate) fn action(
        &self,
    ) -> Result<crate::models::iam::policy::Action, crate::YorishiroError> {
        crate::models::iam::policy::Action::parse(&self.action).ok_or_else(|| {
            crate::YorishiroError::ValidationFailed {
                message: "action is not a known qualified action name".into(),
                details: vec![],
                hint: "use a name such as iam:CreateGroup".into(),
            }
        })
    }
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct SimulatePrincipalPolicyResponse {
    pub decision: crate::models::iam::policy::Decision,
    /// The names of the policies whose statements decided the request.
    pub matched_policies: Vec<String>,
}

impl From<crate::models::iam::access::Simulation> for SimulatePrincipalPolicyResponse {
    fn from(simulation: crate::models::iam::access::Simulation) -> Self {
        Self {
            decision: simulation.decision,
            matched_policies: simulation.matched_policies,
        }
    }
}

crate::dtos::common::list_params_from!(ListAudit, ListIam);
