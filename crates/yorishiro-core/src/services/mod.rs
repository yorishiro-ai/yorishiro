//! Domain logic that isn't itself a record's CRUD: API-key auth/authorization and the
//! embeddings pipeline (provider abstraction, ONNX/OpenAI-compatible implementations, and the
//! sync job that keeps `entities.embedding` current).

pub mod auth;
pub mod embedding;
pub mod marketplace;
pub mod official_templates;
pub mod queue;
