use serde::{Deserialize, Serialize};

use crate::edition::ee::models::inference_jobs::InferenceJobStatus;

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct InferFillResponse {
    pub(crate) job_id: String,
    pub(crate) status: InferFillStatus,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub(crate) enum InferFillStatus {
    Queued,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct InferJobStatusResponse {
    pub(crate) job_id: String,
    pub(crate) status: InferenceJobStatus,
    pub(crate) applied: Option<i64>,
    pub(crate) proposed: Option<i64>,
    pub(crate) skipped: Option<i64>,
    pub(crate) error: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = LlmKeyRequest))]
pub(crate) struct SetLlmKeyRequest {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Stored as given and never returned.
    pub(crate) api_key: String,
}
