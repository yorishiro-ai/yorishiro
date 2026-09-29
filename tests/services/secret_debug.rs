use std::fmt;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tracing_subscriber::fmt::MakeWriter;
use uuid::Uuid;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{workspace_embedding_keys, workspace_llm_keys};

const LLM_SECRET: &str = "issue-492-llm-sentinel-8f01";
const EMBEDDING_SECRET: &str = "issue-492-embedding-sentinel-4c72";

struct Nested<T>(T);

impl<T: fmt::Debug> fmt::Debug for Nested<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Nested").field(&self.0).finish()
    }
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for SharedWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl std::io::Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn llm_model() -> workspace_llm_keys::Model {
    workspace_llm_keys::Model {
        workspace_id: Uuid::nil(),
        base_url: "https://example.test/v1".into(),
        model: "test-model".into(),
        api_key: LLM_SECRET.into(),
        created_at: Utc::now().into(),
        updated_at: Utc::now().into(),
    }
}

fn embedding_model() -> workspace_embedding_keys::Model {
    workspace_embedding_keys::Model {
        workspace_id: Uuid::nil(),
        base_url: "https://example.test/v1".into(),
        model: "embedding-model".into(),
        api_key: EMBEDDING_SECRET.into(),
        dimensions: 768,
        send_dimensions_param: true,
        created_at: Utc::now().into(),
        updated_at: Utc::now().into(),
    }
}

fn assert_safe(output: &str, secret: &str) {
    assert!(!output.contains(secret), "secret leaked in: {output}");
    assert!(
        !output.contains(&secret[..12]),
        "secret fragment leaked in: {output}"
    );
}

#[test]
fn direct_and_nested_debug_redact_both_generated_models() {
    let llm = format!("{:?}", llm_model());
    let nested_llm = format!("{:?}", Nested(llm_model()));
    let embedding = format!("{:?}", embedding_model());
    let nested_embedding = format!("{:?}", Nested(embedding_model()));

    for output in [llm, nested_llm] {
        assert_safe(&output, LLM_SECRET);
    }
    for output in [embedding, nested_embedding] {
        assert_safe(&output, EMBEDDING_SECRET);
    }

    let llm_json = serde_json::to_string(&llm_model()).expect("LLM model serializes");
    let embedding_json =
        serde_json::to_string(&embedding_model()).expect("embedding model serializes");
    assert_safe(&llm_json, LLM_SECRET);
    assert_safe(&embedding_json, EMBEDDING_SECRET);
}

#[test]
fn tracing_and_error_debug_paths_redact_generated_models() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(SharedWriter(output.clone()))
        .finish();

    let error = YorishiroError::Internal(anyhow::anyhow!("provider context: {:?}", llm_model()));
    let error_output = format!("{error:?} {error}");
    assert_safe(&error_output, LLM_SECRET);

    tracing::subscriber::with_default(subscriber, || {
        tracing::error!(model = ?embedding_model(), "provider configuration failed");
    });

    let log_output = String::from_utf8(output.lock().expect("log buffer lock").clone())
        .expect("captured logs are UTF-8");
    assert_safe(&log_output, EMBEDDING_SECRET);
}
