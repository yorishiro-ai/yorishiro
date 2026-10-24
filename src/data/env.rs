//! Application-owned environment keys used outside Loco's YAML renderer.
//!
//! Loco owns the YAML-backed settings and its CLI flags. This enum keeps the few direct reads discoverable without caching values that must be read per request.

use std::ffi::OsString;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Env {
    DatabaseUrl,
    TestTopology,
    Home,
}

#[derive(Clone, Copy)]
pub(crate) enum RetiredEnv {
    OnnxModelPath,
    OnnxTokenizerPath,
    LocalModelPath,
    LocalTokenizerPath,
    OnnxMaxSequenceLength,
    OnnxPooling,
    OnnxQueryInstruction,
}

impl RetiredEnv {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::OnnxModelPath => "YORISHIRO_ONNX_MODEL_PATH",
            Self::OnnxTokenizerPath => "YORISHIRO_ONNX_TOKENIZER_PATH",
            Self::LocalModelPath => "YORISHIRO_LOCAL_MODEL_PATH",
            Self::LocalTokenizerPath => "YORISHIRO_LOCAL_TOKENIZER_PATH",
            Self::OnnxMaxSequenceLength => "YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH",
            Self::OnnxPooling => "YORISHIRO_ONNX_POOLING",
            Self::OnnxQueryInstruction => "YORISHIRO_ONNX_QUERY_INSTRUCTION",
        }
    }

    pub(crate) fn is_set(self) -> bool {
        std::env::var_os(self.name()).is_some()
    }
}

impl Env {
    const fn name(self) -> &'static str {
        match self {
            Self::DatabaseUrl => "DATABASE_URL",
            Self::TestTopology => "YORISHIRO_TEST_TOPOLOGY",
            Self::Home => "HOME",
        }
    }

    pub(crate) fn read(self) -> Result<String, std::env::VarError> {
        std::env::var(self.name())
    }

    pub(crate) fn read_os(self) -> Option<OsString> {
        std::env::var_os(self.name())
    }
}
