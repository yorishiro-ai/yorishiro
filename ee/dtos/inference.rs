use serde::{Deserialize, Serialize};

use crate::edition::ee::models::inference_jobs::InferenceJobStatus;

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct InferFillResponse {
    pub(crate) job_id: String,
    pub(crate) status: InferFillStatus,
}

#[derive(Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InferFillStatus {
    Queued,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct InferJobStatusResponse {
    pub(crate) job_id: String,
    pub(crate) status: InferenceJobStatus,
    pub(crate) applied: Option<i64>,
    pub(crate) proposed: Option<i64>,
    pub(crate) skipped: Option<i64>,
    pub(crate) error: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "LlmKeyRequest")]
pub(crate) struct SetLlmKeyRequest {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Stored as given and never returned.
    pub(crate) api_key: String,
}
