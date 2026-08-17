use super::*;

/// The defaults encode the original single-hop behaviour.
/// `depth` in particular is what keeps an unparameterised recall from fanning out across the graph.
#[test]
fn the_default_recall_query_is_a_single_hop() {
    let query = RecallQuery::default();

    assert_eq!(query.depth, DEFAULT_RECALL_DEPTH);
    assert_eq!(query.depth, 1);
    assert_eq!(query.limit, DEFAULT_RECALL_LIMIT);
    assert!(!query.full);
}

// `MAX_RECALL_DEPTH` is deliberately not asserted here.
// Its value only reaches behaviour through the clamp in `recall_context`, and `depth_beyond_the_maximum_is_clamped_not_rejected` in `tests/repositories/recall/mod.rs` walks that against a real graph.
// An assertion on the constant's range would restate the number without proving anything that test does not.
