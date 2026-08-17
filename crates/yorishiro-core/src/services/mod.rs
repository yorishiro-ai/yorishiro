//! Domain logic that isn't itself a record's CRUD: API-key auth/authorization and the embeddings pipeline (provider abstraction, ONNX/OpenAI-compatible implementations, and the sync job that keeps `entities.embedding` current).

pub mod auth;
pub mod chunking;
pub mod db_load_guard;
pub mod embedding;
pub mod queue;
