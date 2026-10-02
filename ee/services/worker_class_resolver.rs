//! This crate's `WorkerClassResolver`: a workspace with its own row in `workspace_worker_classes` pins its embedding-sync jobs to that class instead of `WorkerClass::Shared`.

use crate::error::YorishiroError;
use crate::workers::embedding_sync::{WorkerClass, WorkerClassResolver};
use async_trait::async_trait;
use uuid::Uuid;

use crate::ee::models::worker_classes;

pub struct WorkerClassAssignmentResolver;

#[async_trait]
impl WorkerClassResolver for WorkerClassAssignmentResolver {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        let explicit = worker_classes::get(conn, workspace_id).await?;
        if explicit.is_some() {
            return Ok(explicit);
        }
        // Provider-neutral demand observation and actual-cost charging are not available through
        // Loco's queue abstraction yet, so policy-derived routing remains an explicit foundation.
        // Returning None makes the caller's Shared fallback honest instead of presenting zeros as observations.
        let _ = conn;
        let _ = workspace_id;
        Ok(None)
    }
}
