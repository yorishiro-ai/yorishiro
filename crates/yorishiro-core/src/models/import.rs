use serde::Serialize;
use utoipa::ToSchema;

/// Outcome of a successful `import_jsonl` call: how many records of each kind were inserted.
/// Since the whole import runs in one transaction (see `import_jsonl`), a non-empty `errors` here never coexists with partially-applied data: either every record listed by `errors` (and everything after it) was rolled back along with it, or `errors` is empty and every counted record is committed.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct ImportResult {
    pub schemas: u64,
    pub entities: u64,
    pub relations: u64,
    pub errors: Vec<String>,
}

#[cfg(test)]
#[path = "../../tests/models/import.rs"]
mod tests;
