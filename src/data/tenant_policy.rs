use std::sync::Arc;

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
