//! Background work: the jobs themselves, and the machinery that routes, runs and recovers them.
//!
//! No worker sets Loco's `max_retries`: a failed job stays `failed` on its lifecycle row and is re-run explicitly, for example by `resync_embeddings`.

// Jobs: one file per kind of background work.
pub mod embedding_sync;
pub mod fill_defaults;
pub(crate) mod import;
pub mod query_embedding;
pub mod reindex;
pub mod startup_reindex;
pub(crate) mod undo;

// Runtime: worker targets and roles, queue policy, dispatch and lifecycle bookkeeping shared by every job.
pub mod dispatch;
pub mod lifecycle;
pub mod queue;
pub mod registry;
pub mod start;
pub mod target;
