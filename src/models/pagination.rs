//! The `limit`/`offset` pair every `list` function takes, and the clamp it's checked against.
//!
//! Previously duplicated per table: `content_entities` and `content_relations` each defined their own `DEFAULT_LIST_LIMIT` (same value, `50`) and their own `.clamp(1, 200)` call, and `marketplace::list_marketplace` imported `content_entities`' copy rather than having a shared one of its own to reach for — a concept a specific table's module happened to own first.

/// Applies to every paginated list, not `search.rs`'s `DEFAULT_SEARCH_LIMIT`: vector/trigram
/// search is a different kind of query (ranked by relevance, not a page over a stable order) and
/// keeps its own, deliberately smaller, default.
pub const DEFAULT_LIST_LIMIT: i64 = 50;

pub const MAX_LIST_LIMIT: i64 = 200;

/// `limit`/`offset`, clamped to `[1, MAX_LIST_LIMIT]` and `[0, ∞)` respectively.
/// A table's own `ListXQuery` embeds this alongside its own filters, rather than redeclaring
/// `limit`/`offset` itself.
#[derive(Debug, Clone, Copy)]
pub struct ListParams {
    pub limit: i64,
    pub offset: i64,
}

impl Default for ListParams {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIST_LIMIT,
            offset: 0,
        }
    }
}

impl ListParams {
    /// Builds directly from a caller's optional `limit`/`offset` (e.g. an Axum `Query` extractor),
    /// applying `Default`'s values where the caller passed neither.
    pub fn new(limit: Option<i64>, offset: Option<i64>) -> Self {
        Self {
            limit: limit.unwrap_or(DEFAULT_LIST_LIMIT),
            offset: offset.unwrap_or(0),
        }
    }

    /// `limit` clamped to `[1, MAX_LIST_LIMIT]`.
    pub fn limit(&self) -> i64 {
        self.limit.clamp(1, MAX_LIST_LIMIT)
    }

    /// `offset` clamped to `[0, ∞)`.
    pub fn offset(&self) -> i64 {
        self.offset.max(0)
    }
}
