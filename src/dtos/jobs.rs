//! Acceptance response shared by queued bulk operations.

use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct QueuedJobResponse {
    pub(crate) job_id: Uuid,
}
