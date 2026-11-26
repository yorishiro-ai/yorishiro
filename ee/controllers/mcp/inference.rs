//! MCP tools for LLM-assisted fill: starting an inference job and reviewing, confirming or rejecting its proposals.
//! These tools are licence-gated like their REST routes.

use crate::controllers::mcp::{YorishiroMcpServer, err_to_tool_result, ok_json, render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::edition::ee::dtos::inference::{
    InferFillResponse, InferFillStatus, InferJobStatusResponse,
};
use crate::edition::ee::models::{inference_jobs, inference_proposals};

pub(super) const TOOL_NAMES: &[&str] = &[
    "create_inference_job",
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
    inference_job_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfirmationJobArgs {
    confirmation_job_id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_inference)]
impl YorishiroMcpServer {
    #[tool(
        description = "Queue inference for missing fields. Poll get_inference_job, then review proposals before confirming them."
    )]
    pub(crate) async fn create_inference_job(
        &self,
        Parameters(input): Parameters<InferArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match crate::edition::ee::workers::infer_fill::enqueue_infer_fill(
            self.app_context(),
            &actor,
            input.schema_name,
        )
        .await
        {
            Ok(job_id) => ok_json(InferFillResponse {
                inference_job_id: job_id,
                status: InferFillStatus::Queued,
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(description = "Poll an inference job in the selected workspace.")]
    pub(crate) async fn get_inference_job(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match inference_jobs::get_as(&self.app_context().db, &actor, input.inference_job_id).await {
            Ok(record) => {
                let completed = record.status == inference_jobs::InferenceJobStatus::Completed;
                ok_json(InferJobStatusResponse {
                    inference_job_id: input.inference_job_id.to_string(),
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

    #[tool(description = "Review proposed inference values without applying them.")]
    pub(crate) async fn list_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            inference_proposals::for_job_as(&self.app_context().db, &actor, input.inference_job_id)
                .await,
        )
    }

    #[tool(
        description = "Queue application of reviewed inference proposals. Poll get_inference_confirmation for the result."
    )]
    pub(crate) async fn confirm_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        match crate::edition::ee::workers::confirm_proposals::enqueue(
            self.app_context(),
            &actor,
            input.inference_job_id,
        )
        .await
        {
            Ok(job_id) => ok_json(
                crate::edition::ee::dtos::inference::ConfirmationJobAccepted {
                    confirmation_job_id: job_id,
                },
            ),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(description = "Mark pending inference proposals as rejected.")]
    pub(crate) async fn reject_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            inference_proposals::reject_as(&self.app_context().db, &actor, input.inference_job_id)
                .await,
        )
    }

    #[tool(description = "Discard inference proposals without applying them.")]
    pub(crate) async fn discard_inference_proposals(
        &self,
        Parameters(input): Parameters<InferenceJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            inference_proposals::discard_as(&self.app_context().db, &actor, input.inference_job_id)
                .await,
        )
    }

    #[tool(description = "Poll for the durable result of an inference confirmation.")]
    pub(crate) async fn get_inference_confirmation(
        &self,
        Parameters(input): Parameters<ConfirmationJobArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let actor = verified!(self, parts);
        render(
            inference_jobs::confirmation_status_as(
                &self.app_context().db,
                &actor,
                input.confirmation_job_id,
            )
            .await
            .map(
                |status| crate::edition::ee::dtos::inference::ConfirmationJobStatus {
                    confirmation_job_id: input.confirmation_job_id,
                    status,
                },
            ),
        )
    }
}
