use super::*;

/// The semaphore bounding concurrent embedding syncs is a private field, so this bound could not
/// be asserted from an external test at all. It is what stops a burst of entity writes from
/// opening an unbounded number of provider calls and exhausting the pool.
#[test]
fn the_embedding_concurrency_bound_is_positive_and_modest() {
    assert!(std::hint::black_box(EMBEDDING_SYNC_MAX_CONCURRENCY) > 0);
    assert!(
        std::hint::black_box(EMBEDDING_SYNC_MAX_CONCURRENCY) <= 16,
        "an unbounded-looking cap would let embedding work starve the connection pool"
    );
}
