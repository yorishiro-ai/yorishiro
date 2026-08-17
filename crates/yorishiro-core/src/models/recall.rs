use serde::Serialize;
use utoipa::ToSchema;

use crate::models::entities::EntityRecord;
use crate::models::relations::DEFAULT_NEIGHBORS_LIMIT;

pub const DEFAULT_RECALL_LIMIT: i64 = DEFAULT_NEIGHBORS_LIMIT;

/// Default number of hops `recall_context` traverses when `depth` is omitted: matches the original single-hop behavior so existing callers are unaffected.
pub const DEFAULT_RECALL_DEPTH: i64 = 1;

/// Upper bound on `depth`, clamped in `recall_context`, to prevent runaway fan-out queries on dense graphs (each additional hop can multiply the number of neighbor fetches).
pub const MAX_RECALL_DEPTH: i64 = 3;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecallRelation {
    pub relation_type: String,
    pub direction: String,
    /// The connected entity.
    /// Shallow (only `x-embed` fields in `data`) by default; pass `full: true` to `recall_context` to get every field instead.
    pub neighbor: EntityRecord,
    /// How many hops away from the requested entity this neighbor is (1 = direct neighbor, 2 = neighbor-of-neighbor, etc).
    pub hop_distance: i32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecallContext {
    /// The requested entity, always with its full `data`.
    pub entity: EntityRecord,
    /// Flat list of every neighbor found within `depth` hops, each tagged with its `hop_distance`.
    /// A neighbor reachable via more than one path is only reported once, at the shortest hop_distance it was found at.
    pub relations: Vec<RecallRelation>,
    /// `true` when more neighbors exist at some hop than `limit` allowed to be included there.
    pub truncated: bool,
}

/// Parameters for [`crate::repositories::recall::recall_context`].
#[derive(Debug, Clone, Copy)]
pub struct RecallQuery {
    /// Maximum number of relations/neighbors to fetch per hop.
    pub limit: i64,
    /// When true, neighbor entities include every field instead of only `x-embed` fields.
    pub full: bool,
    /// How many hops to traverse outward from the requested entity.
    /// Clamped to `[1, MAX_RECALL_DEPTH]`; defaults to `DEFAULT_RECALL_DEPTH` (1, the original single-hop behavior).
    pub depth: i64,
}

impl Default for RecallQuery {
    fn default() -> Self {
        Self {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            depth: DEFAULT_RECALL_DEPTH,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/models/recall.rs"]
mod tests;
