#[cfg(feature = "test-support")]
pub mod dispatch;
#[cfg(not(feature = "test-support"))]
pub(crate) mod dispatch;
pub mod embedding_sync;
pub(crate) mod lifecycle;
pub(crate) mod queue;
pub mod reindex;
