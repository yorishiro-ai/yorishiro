use serde::Deserialize;

use crate::workers::embedding_sync::WorkerClass;

#[derive(Debug, Deserialize)]
pub(crate) struct SetWorkerClassRequest {
    pub(crate) worker_class: WorkerClass,
}
