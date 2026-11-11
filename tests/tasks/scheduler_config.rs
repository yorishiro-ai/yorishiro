use std::path::Path;

/// The index maintenance task only runs when the deployed scheduler lists it.
/// The configs are Tera templates, so the check reads the text rather than parsing YAML.
#[test]
fn deployed_schedulers_run_the_embedding_index_maintenance_task() {
    for file in ["production.yaml", "development.yaml"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("run: \"embedding_index_maintenance\""),
            "{file} must schedule the embedding index maintenance task"
        );
    }
}
