use serde::{Deserialize, Serialize};

use crate::ee::models::inference_jobs::InferenceJobStatus;

#[derive(Debug, Serialize)]
pub(crate) struct InferFillResponse {
    pub(crate) job_id: String,
    pub(crate) status: InferenceJobStatus,
}

#[derive(Debug, Serialize)]
pub(crate) struct InferJobStatusResponse {
    pub(crate) job_id: String,
    pub(crate) status: InferenceJobStatus,
    pub(crate) applied: Option<i64>,
    pub(crate) proposed: Option<i64>,
    pub(crate) skipped: Option<i64>,
    pub(crate) error: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SetLlmKeyRequest {
    pub(crate) base_url: String,
    pub(crate) model: String,
    /// Stored as given and never returned.
    pub(crate) api_key: String,
}
