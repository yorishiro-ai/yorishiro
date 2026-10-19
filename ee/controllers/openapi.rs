//! OpenAPI names refer to the actual serialized request and response types.

pub(crate) use crate::edition::ee::dtos::{
    dashboard::TenantOverview,
    embedding::SetEmbeddingKeyRequest as EmbeddingKeyRequest,
    entity_columns::SetColumnsRequest,
    inference::{InferFillResponse, InferJobStatusResponse, SetLlmKeyRequest as LlmKeyRequest},
    marketplace::{ForkResponse, PublishVersionRequest, SetVisibilityRequest},
    oauth::OAuthStatus,
    origin::MergeResponse,
    worker_class::SetWorkerClassRequest as WorkerClassRequest,
};
pub(crate) use crate::edition::ee::models::{
    entity_column_preferences::ColumnPreference,
    inference_proposals::{
        ProposalActionReport as ProposalActionResponse, ProposalRecord as InferenceProposalResponse,
    },
    marketplace::{
        MarketplaceListing, SubmitReviewRequest, TemplateReviewRecord, TemplateVersionRecord,
    },
    schema_schemas::merge::MergePlan,
    workspace_embedding_keys::EmbeddingKeyDescription as EmbeddingKeyResponse,
    workspace_llm_keys::LlmKeyDescription as LlmKeyResponse,
    workspace_schema_forks::{
        CreateInput as CreateForkRequest, ForkRecord, UpdateInput as UpdateForkRequest,
    },
    workspace_worker_classes::WorkerClassAssignment as WorkerClassResponse,
};
pub(crate) use crate::models::schema_schemas::UpstreamChange;
