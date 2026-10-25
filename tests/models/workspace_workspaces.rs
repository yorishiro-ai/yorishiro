use yorishiro::models::workspace_workspaces::StartupReindexRow;

#[test]
fn only_a_differently_stamped_workspace_needs_a_reindex() {
    let row = |model: Option<&str>, dimensions: Option<i32>| StartupReindexRow {
        id: uuid::Uuid::nil(),
        embedding_model: model.map(str::to_owned),
        embedding_dimensions: dimensions,
        embedding_reindexing: false,
    };
    assert!(row(Some("old-model"), Some(768)).is_stamped_with_other_embedding("new-model", 768));
    assert!(!row(Some("new-model"), Some(768)).is_stamped_with_other_embedding("new-model", 768));
    assert!(row(Some("new-model"), Some(1536)).is_stamped_with_other_embedding("new-model", 768));
    // An unstamped workspace has no vectors to replace.
    assert!(!row(None, None).is_stamped_with_other_embedding("new-model", 768));
    assert!(
        StartupReindexRow {
            embedding_reindexing: true,
            ..row(Some("new-model"), Some(768))
        }
        .is_stamped_with_other_embedding("new-model", 768)
    );
}
