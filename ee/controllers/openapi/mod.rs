//! `OpenAPI` names refer to the actual serialized request and response types.

pub(crate) use crate::edition::ee::dtos::{
    embedding::SetEmbeddingKeyRequest as EmbeddingKeyRequest,
    inference::SetLlmKeyRequest as LlmKeyRequest, marketplace::PublishVersionRequest,
    worker_class::SetWorkerClassRequest as WorkerClassRequest,
};
pub(crate) use crate::edition::ee::models::{
    inference_proposals::{
        ProposalActionReport as ProposalActionResponse, ProposalRecord as InferenceProposalResponse,
    },
    workspace_embedding_keys::EmbeddingKeyDescription as EmbeddingKeyResponse,
    workspace_llm_keys::LlmKeyDescription as LlmKeyResponse,
    workspace_schema_forks::{
        CreateInput as CreateForkRequest, ForkRecord, UpdateInput as UpdateForkRequest,
    },
    workspace_worker_classes::WorkerClassAssignment as WorkerClassResponse,
};
