use super::*;

/// An import that touched nothing must report zeros and no errors rather than, say, a default that looks like a partial success.
#[test]
fn a_default_result_reports_nothing_imported_and_no_errors() {
    let result = ImportResult::default();

    assert_eq!(result.schemas, 0);
    assert_eq!(result.entities, 0);
    assert_eq!(result.relations, 0);
    assert!(result.errors.is_empty());
}

/// The counts are what a caller reports back to a user, so the serialised field names are part of the API and pinned here.
#[test]
fn the_serialised_shape_names_each_counted_kind() {
    let result = ImportResult {
        schemas: 1,
        entities: 2,
        relations: 3,
        errors: vec!["line 4: bad".into()],
    };

    let json = serde_json::to_value(&result).unwrap();

    assert_eq!(json["schemas"], 1);
    assert_eq!(json["entities"], 2);
    assert_eq!(json["relations"], 3);
    assert_eq!(json["errors"][0], "line 4: bad");
}
