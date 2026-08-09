use super::*;

/// The defaults encode the original single-hop behaviour. `depth` in particular is what keeps an
/// unparameterised recall from fanning out across the graph.
#[test]
fn the_default_recall_query_is_a_single_hop() {
    let query = RecallQuery::default();

    assert_eq!(query.depth, DEFAULT_RECALL_DEPTH);
    assert_eq!(query.depth, 1);
    assert_eq!(query.limit, DEFAULT_RECALL_LIMIT);
    assert!(!query.full);
}

/// The clamp bound is public because callers validate against it before issuing a query; it must
/// stay a sane range rather than silently allowing an unbounded traversal.
#[test]
fn the_depth_bound_is_a_usable_range() {
    assert!(std::hint::black_box(MAX_RECALL_DEPTH) >= DEFAULT_RECALL_DEPTH);
    assert!(std::hint::black_box(MAX_RECALL_DEPTH) <= 3);
}
