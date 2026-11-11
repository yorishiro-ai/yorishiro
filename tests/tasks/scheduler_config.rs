use std::path::Path;

/// The index maintenance task only runs when the deployed scheduler lists it.
#[test]
fn deployed_schedulers_run_the_embedding_index_maintenance_task() {
    for file in ["production.yaml", "development.yaml"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join(file);
        let config: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let jobs = &config["scheduler"]["jobs"];
        assert_eq!(
            jobs["embedding_index_maintenance"]["run"].as_str(),
            Some("embedding_index_maintenance"),
            "{file} must schedule the embedding index maintenance task"
        );
    }
}
