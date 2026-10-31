//! Application-owned environment keys used outside Loco's YAML renderer.
//!
//! Loco owns the YAML-backed settings and its CLI flags. This enum keeps the few direct reads discoverable without caching values that must be read per request.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Env {
    DatabaseUrl,
    LocoConfigFolder,
    TestTopology,
}

impl Env {
    const fn name(self) -> &'static str {
        match self {
            Self::DatabaseUrl => "DATABASE_URL",
            Self::LocoConfigFolder => "LOCO_CONFIG_FOLDER",
            Self::TestTopology => "YORISHIRO_TEST_TOPOLOGY",
        }
    }

    pub(crate) fn read(self) -> Result<String, std::env::VarError> {
        std::env::var(self.name())
    }

    pub(crate) fn read_os(self) -> Option<std::ffi::OsString> {
        std::env::var_os(self.name())
    }
}
