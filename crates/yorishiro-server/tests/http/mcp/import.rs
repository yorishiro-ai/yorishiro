use super::*;

/// The import tool takes the JSONL document as one string.
/// It is required: an omitted body is a client error, not an empty import that silently reports success.
#[test]
fn the_document_is_required() {
    assert!(serde_json::from_value::<ImportJsonlArgs>(serde_json::json!({})).is_err());

    let args: ImportJsonlArgs =
        serde_json::from_value(serde_json::json!({ "jsonl": "{\"kind\":\"entity\"}" })).unwrap();
    assert!(args.jsonl.contains("entity"));
}

/// Newlines are the record separator, so they must survive deserialization intact rather than being normalised away.
#[test]
fn newlines_in_the_document_are_preserved() {
    let args: ImportJsonlArgs =
        serde_json::from_value(serde_json::json!({ "jsonl": "line1\nline2\nline3" })).unwrap();

    assert_eq!(args.jsonl.lines().count(), 3);
}
