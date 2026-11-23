use yorishiro::controllers::openapi::operation_id;

#[test]
fn operation_ids_are_stable_and_path_safe() {
    assert_eq!(
        operation_id("get", "/api/entities/{id}"),
        "get_api_entities_id"
    );
    assert_eq!(operation_id("post", "/setup"), "post_setup");
}
