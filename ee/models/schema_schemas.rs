//! Enterprise schema operations layered over the community schema model.

pub mod merge;
mod origin;
mod query;

pub(crate) use merge::MergePlan;
pub(crate) use origin::{merge_apply, merge_preview};
pub(crate) use query::list_with_upstream_changes;
