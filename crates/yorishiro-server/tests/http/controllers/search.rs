use super::*;

/// The REST search endpoint deserializes its parameters from the query string. Only
/// `query_text` is required; the rest narrow an already-valid search, so a caller sending just
/// the text must succeed.
#[test]
fn only_the_query_text_is_required() {
    let params: SearchEntitiesParams =
        serde_json::from_value(serde_json::json!({ "query_text": "how do I deploy" })).unwrap();

    assert_eq!(params.query_text, "how do I deploy");
    assert!(params.entity_type.is_none());
    assert!(params.filter.is_none());
}

/// A search with no text is meaningless -- it would embed to noise -- so the parameter is
/// required rather than defaulted to an empty string.
#[test]
fn a_search_without_text_is_rejected() {
    assert!(
        serde_json::from_value::<SearchEntitiesParams>(
            serde_json::json!({ "entity_type": "task" })
        )
        .is_err()
    );
}

/// The filter arrives as a raw string on the query string and is parsed separately by
/// `parse_filter_param`; at this layer it stays a string so the endpoint can report a precise
/// error for malformed JSON.
#[test]
fn the_filter_is_carried_as_a_raw_string_for_later_parsing() {
    let params: SearchEntitiesParams = serde_json::from_value(serde_json::json!({
        "query_text": "anything",
        "filter": "{\"status\":\"active\"}"
    }))
    .unwrap();

    assert!(params.filter.is_some());
}
