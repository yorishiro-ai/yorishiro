//! This crate's `WorkerClassResolver`: a workspace with its own row in `identity_workspace_worker_classes` pins its embedding-sync jobs to that class instead of `WorkerClass::Shared`.

use async_trait::async_trait;
use uuid::Uuid;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::workers::embedding_sync::{WorkerClass, WorkerClassResolver};

use crate::models::worker_classes;

pub struct WorkerClassAssignmentResolver;

#[async_trait]
impl WorkerClassResolver for WorkerClassAssignmentResolver {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<WorkerClass>, YorishiroError> {
        worker_classes::get(conn, workspace_id).await
    }
}
