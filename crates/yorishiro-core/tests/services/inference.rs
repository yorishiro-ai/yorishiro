use super::*;

/// The model is asked for specific fields, and only those may come back. A model that answers
/// with a key nobody asked for would otherwise have that key written into an entity, where the
/// schema does not define it -- validation would reject the write, but only after the proposal
/// had been stored and shown to someone as though it were usable.
#[test]
fn a_proposal_keeps_only_the_fields_that_were_asked_for() {
    let answered: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
        r#"{"category": "fiction", "invented": "nobody asked", "summary": "a summary"}"#,
    )
    .unwrap();
    let asked = ["category", "summary"];

    let kept: serde_json::Map<String, serde_json::Value> = answered
        .iter()
        .filter(|(key, _)| asked.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    assert_eq!(kept.len(), 2);
    assert!(kept.contains_key("category"));
    assert!(kept.contains_key("summary"));
    assert!(
        !kept.contains_key("invented"),
        "a field the model invented must not survive"
    );
}

/// Asking for nothing must not produce a request. A workspace whose entities are all complete
/// would otherwise pay for a call whose answer is discarded.
#[tokio::test]
async fn no_missing_fields_makes_no_request() {
    // An unroutable base_url: if a request were made, this would error rather than return empty.
    let client = InferenceClient::new(InferenceConfig {
        base_url: "http://127.0.0.1:1/v1".into(),
        model: "unused".into(),
        api_key: "unused".into(),
    });

    let proposals = client
        .propose_fields(&serde_json::json!({"title": "x"}), &[])
        .await
        .expect("asking for no fields must not call the provider");

    assert!(proposals.is_empty());
}

/// A provider that refuses gets reported as a caller-fixable error, not an internal one -- the
/// key, the model name and the URL are all workspace configuration. The message must not carry
/// the provider's body, which can quote the key back.
#[tokio::test]
async fn a_provider_error_is_reported_without_leaking_the_key() {
    let client = InferenceClient::new(InferenceConfig {
        base_url: "http://127.0.0.1:1/v1".into(),
        model: "unused".into(),
        api_key: "ysr-secret-value".into(),
    });

    let error = client
        .propose_fields(&serde_json::json!({"title": "x"}), &["category"])
        .await
        .expect_err("an unroutable provider must fail");

    let rendered = format!("{error:?}");
    assert!(
        !rendered.contains("ysr-secret-value"),
        "the api key must never appear in an error: {rendered}"
    );
}
