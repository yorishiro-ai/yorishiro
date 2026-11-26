//! The `limit`/`offset` pair every `list` function takes, and the clamp it's checked against.
//!
//! One definition serves every table, because no single table owns pagination.

/// Applies to every paginated list.
/// Search ranks by relevance rather than paging over a stable order, so it keeps its own smaller default.
pub(crate) const DEFAULT_LIST_LIMIT: i64 = 50;

pub const MAX_LIST_LIMIT: i64 = 200;

/// `limit`/`offset`, clamped to `[1, MAX_LIST_LIMIT]` and `[0, ∞)` respectively.
/// A table's own `ListXQuery` embeds this alongside its own filters.
#[derive(Clone, Copy)]
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
    /// Builds from a caller's optional `limit` and `offset`, using the defaults where absent.
    #[must_use]
    pub fn new(limit: Option<i64>, offset: Option<i64>) -> Self {
        Self {
            limit: limit.unwrap_or(DEFAULT_LIST_LIMIT),
            offset: offset.unwrap_or(0),
        }
    }

    /// `limit` clamped to `[1, MAX_LIST_LIMIT]`.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit.clamp(1, MAX_LIST_LIMIT).cast_unsigned()
    }

    /// `offset` clamped to `[0, ∞)`.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset.max(0).cast_unsigned()
    }

    /// Builds from Loco's 1-based `PaginationQuery`.
    /// `page` is clamped to at least 1, and `page_size` to `[1, MAX_LIST_LIMIT]`.
    pub(crate) fn from_pagination_query(
        page: u64,
        page_size: u64,
    ) -> Result<Self, crate::YorishiroError> {
        let page_size = page_size
            .clamp(1, MAX_LIST_LIMIT.cast_unsigned())
            .cast_signed();
        let offset = i64::try_from(page.max(1) - 1)
            .ok()
            .and_then(|page| page.checked_mul(page_size))
            .ok_or_else(|| crate::YorishiroError::ValidationFailed {
                message: "pagination offset is too large".into(),
                details: vec![],
                hint: "use a smaller page number".into(),
            })?;
        Ok(Self {
            limit: page_size,
            offset,
        })
    }
}
