use super::*;

/// `init` returns a guard whose drop flushes buffered output. Holding it for the process
/// lifetime is the caller's contract, so the type has to be nameable and movable -- if it stopped
/// being returned by value, `main` could no longer keep it alive across the run.
#[test]
fn the_guard_can_be_held_by_value_for_the_process_lifetime() {
    fn assert_movable<T: Sized>() {}
    assert_movable::<LogGuard>();
}
