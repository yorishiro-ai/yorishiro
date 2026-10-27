//! Runtime environment inputs owned by the integration-test harness.

use std::ffi::OsString;

#[derive(Clone, Copy)]
pub(crate) enum TestEnv {
    DatabaseUrl,
    #[cfg(feature = "enterprise")]
    LocoEnvironment,
    TopologyMarkerDir,
    BackendMarkerDir,
    RequestTimingDir,
    #[cfg(feature = "enterprise")]
    SqliteLockChildUri,
    RedisBaseUrl,
}

impl TestEnv {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::DatabaseUrl => "DATABASE_URL",
            #[cfg(feature = "enterprise")]
            Self::LocoEnvironment => "LOCO_ENV",
            Self::TopologyMarkerDir => "YORISHIRO_TOPOLOGY_MARKER_DIR",
            Self::BackendMarkerDir => "YORISHIRO_BACKEND_MARKER_DIR",
            Self::RequestTimingDir => "YORISHIRO_REQUEST_TIMING_DIR",
            #[cfg(feature = "enterprise")]
            Self::SqliteLockChildUri => "YORISHIRO_SQLITE_LOCK_CHILD_URI",
            Self::RedisBaseUrl => "YORISHIRO_TEST_REDIS_BASE_URL",
        }
    }

    pub(crate) fn read(self) -> Result<String, std::env::VarError> {
        std::env::var(self.name())
    }

    pub(crate) fn read_os(self) -> Option<OsString> {
        std::env::var_os(self.name())
    }
}
