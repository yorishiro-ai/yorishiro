use super::*;

/// The library tool fetches one saved template by id. A malformed id must fail here rather than
/// reaching the repository layer.
#[test]
fn fetching_an_item_requires_a_valid_uuid() {
    assert!(serde_json::from_value::<GetTemplateLibraryItemArgs>(serde_json::json!({})).is_err());
    assert!(
        serde_json::from_value::<GetTemplateLibraryItemArgs>(
            serde_json::json!({ "id": "not-a-uuid" })
        )
        .is_err()
    );

    let args: GetTemplateLibraryItemArgs =
        serde_json::from_value(serde_json::json!({ "id": "00000000-0000-0000-0000-000000000000" }))
            .unwrap();
    assert!(args.id.is_nil());
}
