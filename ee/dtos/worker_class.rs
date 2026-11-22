use serde::Deserialize;

use crate::workers::embedding_sync::WorkerClass;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "WorkerClassRequest")]
pub(crate) struct SetWorkerClassRequest {
    pub(crate) worker_class: WorkerClass,
}
