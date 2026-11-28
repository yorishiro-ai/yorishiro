//! Public response shapes for queued jobs.

use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ImportJobAccepted {
    pub(crate) import_job_id: Uuid,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct ImportJobStatus {
    pub(crate) import_job_id: Uuid,
    #[serde(flatten)]
    pub(crate) status: crate::models::queue_job_lifecycles::BulkJobStatus,
}

#[derive(Serialize, schemars::JsonSchema)]
pub(crate) struct MigrationJobStatus {
    pub(crate) migration_job_id: Uuid,
    #[serde(flatten)]
    pub(crate) status: crate::models::queue_job_lifecycles::BulkJobStatus,
}
