use loco_rs::bgworker::BackgroundWorker;
use yorishiro::ee::workers::infer_fill::InferFillWorker;

#[test]
fn infer_fill_worker_keeps_its_routing_tag_and_class_name() {
    assert_eq!(InferFillWorker::tags(), vec!["infer-fill".to_string()]);
    assert_eq!(InferFillWorker::class_name(), "InferFillWorker");
}
