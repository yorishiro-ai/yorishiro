use super::*;

/// An unparameterised search must stay bounded: the default limit is what stops a query from returning the whole workspace.
#[test]
fn the_default_search_query_is_bounded_and_unfiltered() {
    let query = SearchQuery::default();

    assert!(query.entity_type.is_none());
    assert!(query.filter.is_none());
    assert!(query.limit > 0);
}

/// `distance` is `None` for a hit that came from the pg_trgm fallback rather than from a vector comparison.
/// That distinction is meaningful to a caller ranking results, so it must serialise as an explicit null rather than being omitted.
#[test]
fn a_hit_without_an_embedding_reports_a_null_distance() {
    let hit = SearchHit {
        entity: EntityRecord {
            id: uuid::Uuid::nil(),
            workspace_id: uuid::Uuid::nil(),
            schema_id: uuid::Uuid::nil(),
            schema_version: 1,
            entity_type: "note".into(),
            data: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            created_by: None,
            updated_by: None,
        },
        distance: None,
    };

    let json = serde_json::to_value(&hit).unwrap();

    assert!(json.get("distance").is_some());
    assert!(json["distance"].is_null());
}
