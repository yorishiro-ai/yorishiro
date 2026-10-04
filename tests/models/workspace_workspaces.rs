use yorishiro::models::workspace_workspaces::StartupReindexRow;

#[test]
fn only_a_differently_stamped_workspace_needs_a_reindex() {
    let row = |model: Option<&str>| StartupReindexRow {
        id: uuid::Uuid::nil(),
        embedding_model: model.map(str::to_owned),
    };
    assert!(row(Some("old-model")).is_stamped_with_other_model("new-model"));
    assert!(!row(Some("new-model")).is_stamped_with_other_model("new-model"));
    // An unstamped workspace has no vectors to replace.
    assert!(!row(None).is_stamped_with_other_model("new-model"));
}
