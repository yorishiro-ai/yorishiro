use serde::Deserialize;

use crate::workers::embedding_sync::WorkerClass;

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = WorkerClassRequest))]
pub(crate) struct SetWorkerClassRequest {
    pub(crate) worker_class: WorkerClass,
}
