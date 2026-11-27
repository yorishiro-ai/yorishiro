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

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListApiEvents {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Only the events of this request.
    pub request_id: Option<String>,
}

/// One recorded call: who made it, when, from where, what it was, and how it ended.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ApiEventRecord {
    #[serde(rename = "api_event_id")]
    pub id: Uuid,
    pub request_id: String,
    /// `started` before a write runs, `completed` when any call ends.
    pub phase: String,
    pub event_time: chrono::DateTime<chrono::FixedOffset>,
    pub transport: String,
    /// The REST route or the MCP tool name.
    pub event_name: String,
    pub read_only: bool,
    pub source_ip: Option<String>,
    pub user_agent: String,
    pub tenant_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub access_key_id: Option<Uuid>,
    /// The YRN of the access key the call was made with.
    pub caller: Option<String>,
    pub workspace_id: Option<Uuid>,
    /// The background job this row ends, when it is not a request.
    pub job_id: Option<Uuid>,
    /// The IAM checks that passed, each as `action target-YRN`.
    pub permissions: serde_json::Value,
    /// `success`, `denied` or `failed`.
    pub outcome: Option<String>,
    pub status_code: Option<i32>,
    pub latency_ms: Option<i64>,
}

impl From<crate::models::api_events::Model> for ApiEventRecord {
    fn from(event: crate::models::api_events::Model) -> Self {
        Self {
            id: event.id,
            request_id: event.request_id,
            phase: event.phase,
            event_time: event.created_at,
            transport: event.transport,
            event_name: event.event_name,
            read_only: event.read_only,
            source_ip: event.source_ip,
            user_agent: event.user_agent,
            tenant_id: event.tenant_id,
            user_id: event.user_id,
            access_key_id: event.access_key_id,
            caller: event.caller,
            workspace_id: event.workspace_id,
            job_id: event.job_id,
            permissions: event.permissions,
            outcome: event.outcome,
            status_code: event.status_code,
            latency_ms: event.latency_ms,
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct AuditRecord {
    #[serde(rename = "audit_event_id")]
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

crate::dtos::common::list_params_from!(ListAudit, ListIam, ListApiEvents);
