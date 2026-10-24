use std::sync::Arc;

/// A configured deployment quota, or a backend isolation constraint that cannot be relaxed.
pub(crate) enum TenantLimit {
    Unlimited,
    Configured(i32),
    SingleTenant,
}

impl TenantLimit {
    pub(crate) fn resolve(backend: sea_orm::DatabaseBackend, configured: Option<i32>) -> Self {
        match backend {
            sea_orm::DatabaseBackend::Sqlite => Self::SingleTenant,
            _ => configured.map_or(Self::Unlimited, Self::Configured),
        }
    }

    pub(crate) fn maximum(&self) -> Option<u64> {
        match self {
            Self::Unlimited => None,
            Self::Configured(max) => Some((*max).max(0) as u64),
            Self::SingleTenant => Some(1),
        }
    }

    pub(crate) fn exhausted(&self) -> crate::error::YorishiroError {
        let remedy = match self {
            Self::SingleTenant => {
                "this backend has no database-enforced tenant isolation; use PostgreSQL for more than one tenant"
            }
            _ => "raise YORISHIRO_MAX_TENANTS or delete an existing tenant",
        };
        crate::error::YorishiroError::Conflict {
            message: format!(
                "this deployment has reached its tenant limit ({}); {remedy}",
                self.maximum().unwrap_or_default()
            ),
        }
    }
}

/// Resolves the edition's tenant cap at use time so licence expiry takes effect.
#[derive(Clone)]
pub(crate) struct TenantCap(Arc<dyn Fn() -> Option<i32> + Send + Sync>);

impl TenantCap {
    pub(crate) fn new(resolve: impl Fn() -> Option<i32> + Send + Sync + 'static) -> Self {
        Self(Arc::new(resolve))
    }

    pub(crate) fn resolve(&self) -> Option<i32> {
        (self.0)()
    }
}
