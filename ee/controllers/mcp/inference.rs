//! MCP tools for LLM-assisted fill: starting an inference job and reviewing, confirming or rejecting its proposals.
//! These tools are licence-gated like their REST routes.

use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::controllers::mcp::{YorishiroMcpServer, err_to_tool_result, ok_json, verify};
use crate::edition::ee::dtos::inference::{
    InferFillResponse, InferFillStatus, InferJobStatusResponse,
};
use crate::edition::ee::models::{inference_jobs, inference_proposals};
use crate::models::api_keys::ApiKeyScope;

pub(super) const TOOL_NAMES: &[&str] = &[
    "infer_fill",
    "get_inference_job",
    "list_inference_proposals",
    "confirm_inference_proposals",
    "reject_inference_proposals",
    "discard_inference_proposals",
    "get_inference_confirmation",
];

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InferArgs {
    schema_name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InferenceJobArgs {
    job_id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_inference)]
impl YorishiroMcpServer {
    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Queue inference for missing fields in the selected workspace; poll get_inference_job, then review proposals before confirmation (requires an active licence, schema scope and inference_create IAM permission)")]
    pub(crate) async fn infer_fill(
        &self,
        Parameters(input): Parameters<InferArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("infer_fill")?,
            )
            .await?
        );
        match crate::edition::ee::workers::infer_fill::enqueue_infer_fill(
            self.app_context(),
            &actor,
            input.schema_name,
        )
        .await
        {
            Ok(job_id) => ok_json(InferFillResponse {
                job_id,
                status: InferFillStatus::Queued,
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Poll an inference job in the selected workspace (requires an active licence, read scope and inference_job_get IAM permission)")]
    pub(crate) async fn get_inference_job(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("get_inference_job")?,
            )
            .await?
        );
        match inference_jobs::get_as(&self.app_context().db, &actor, input.job_id).await {
            Ok(record) => {
                let completed = record.status == inference_jobs::InferenceJobStatus::Completed;
                ok_json(InferJobStatusResponse {
                    job_id: input.job_id.to_string(),
                    status: record.status,
                    applied: completed.then_some(record.applied),
                    proposed: completed.then_some(record.proposed),
                    skipped: completed.then_some(record.skipped),
                    error: record.error,
                })
            }
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read), description = "Review proposed inference values without applying them (requires an active licence, read scope and inference_proposal_list IAM permission on the selected workspace)")]
    pub(crate) async fn list_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("list_inference_proposals")?,
            )
            .await?
        );
        render(inference_proposals::for_job_as(&self.app_context().db, &actor, input.job_id).await)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Queue application of reviewed inference proposals; poll get_inference_confirmation with the returned job_id (requires an active licence, schema scope and inference_confirm IAM permission on the selected workspace)")]
    pub(crate) async fn confirm_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("confirm_inference_proposals")?,
            )
            .await?
        );
        match crate::edition::ee::workers::confirm_proposals::enqueue(
            self.app_context(),
            &actor,
            input.job_id,
        )
        .await
        {
            Ok(job_id) => ok_json(crate::dtos::jobs::QueuedJobResponse { job_id }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Reject pending inference proposals (requires an active licence, schema scope and inference_reject IAM permission on the selected workspace)")]
    pub(crate) async fn reject_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("reject_inference_proposals")?,
            )
            .await?
        );
        render(inference_proposals::reject_as(&self.app_context().db, &actor, input.job_id).await)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Discard inference proposals without applying them (requires an active licence, schema scope and inference_discard IAM permission on the selected workspace)")]
    pub(crate) async fn discard_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("discard_inference_proposals")?,
            )
            .await?
        );
        render(inference_proposals::discard_as(&self.app_context().db, &actor, input.job_id).await)
    }

    #[tool(meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Schema), description = "Poll durable inference confirmation results (requires an active licence, schema scope and inference_confirmation_get IAM permission on the selected workspace)")]
    pub(crate) async fn get_inference_confirmation(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(
            verify(
                self.app_context(),
                &parts,
                self.required_scope("get_inference_confirmation")?,
            )
            .await?
        );
        render(
            inference_jobs::confirmation_status_as(&self.app_context().db, &actor, input.job_id)
                .await,
        )
    }
}
