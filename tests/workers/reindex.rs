use loco_rs::bgworker::BackgroundWorker;
use yorishiro::workers::reindex::{
    ReindexWorkerOfficial, ReindexWorkerShared, ReindexWorkerTenantPrivate,
};

#[test]
fn each_reindex_worker_type_carries_only_its_class_tag() {
    assert_eq!(
        ReindexWorkerTenantPrivate::tags(),
        vec!["worker-class:tenant-private".to_string()]
    );
    assert_eq!(
        ReindexWorkerOfficial::tags(),
        vec!["worker-class:official".to_string()]
    );
    assert_eq!(
        ReindexWorkerShared::tags(),
        vec!["worker-class:shared".to_string()]
    );
}

#[test]
fn reindex_worker_types_have_distinct_queue_class_names() {
    let names = [
        ReindexWorkerTenantPrivate::class_name(),
        ReindexWorkerOfficial::class_name(),
        ReindexWorkerShared::class_name(),
    ];
    assert_eq!(
        names.iter().collect::<std::collections::HashSet<_>>().len(),
        names.len()
    );
}
