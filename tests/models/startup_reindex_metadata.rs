use yorishiro::services::embedding::deployment_embedding_metadata;

#[test]
fn deployment_metadata_matches_provider_resolution_without_loading_provider() {
    let config = config_with_embedding(serde_json::json!({
        "provider": "none",
        "dimensions": 1024,
        "base_url": "http://embedding.test",
        "api_key": "",
        "model": "external-model",
        "send_dimensions_param": false,
        "local_model": "unused",
        "local_max_sequence_length": 512
    }));
    let settings = config
        .settings::<yorishiro::data::settings::Settings>()
        .unwrap();
    let metadata = deployment_embedding_metadata(&settings)
        .unwrap()
        .expect("URL and model select the external provider before the provider enum");
    assert_eq!(metadata.model, "external-model");
    assert_eq!(metadata.dimensions, 1024);
}

#[test]
fn disabled_deployment_has_no_effective_metadata() {
    let settings = config_with_embedding(serde_json::json!({
        "provider": "none",
        "dimensions": 768,
        "base_url": null,
        "api_key": "",
        "model": null,
        "send_dimensions_param": false,
        "local_model": "unused",
        "local_max_sequence_length": 512
    }))
    .settings::<yorishiro::data::settings::Settings>()
    .unwrap();
    assert!(deployment_embedding_metadata(&settings).unwrap().is_none());
}

#[test]
fn local_metadata_uses_provider_model_id_and_declared_width() {
    let settings = config_with_embedding(serde_json::json!({
        "provider": "local",
        "dimensions": 768,
        "base_url": null,
        "api_key": "",
        "model": null,
        "send_dimensions_param": false,
        "local_model": "multilingual-e5-base",
        "local_max_sequence_length": 512
    }))
    .settings::<yorishiro::data::settings::Settings>()
    .unwrap();
    let metadata = deployment_embedding_metadata(&settings)
        .unwrap()
        .expect("development settings select local provider");
    assert_eq!(metadata.model, "intfloat/multilingual-e5-base");
    assert_eq!(metadata.dimensions, 768);
}

fn config_with_embedding(embedding: serde_json::Value) -> loco_rs::config::Config {
    let mut config = loco_rs::environment::Environment::Any("test_sqlite".into())
        .load()
        .expect("load test settings");
    config.settings.as_mut().unwrap()["embedding"] = embedding;
    config
}
