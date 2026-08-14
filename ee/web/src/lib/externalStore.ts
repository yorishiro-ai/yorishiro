/**
 * The `subscribe`/`notify` pair that `useSyncExternalStore` needs, for state that lives outside
 * React entirely -- `localStorage`/`sessionStorage` written through `lib/api`, not a `useState`.
 *
 * Storage writes fire no event in the tab that made them (the `storage` event is other-tabs-only),
 * so a component reading it has nothing to re-render on. `notify()` is what the writer calls to
 * close that gap; `useSyncExternalStore` handles the rest, including tearing under concurrent
 * rendering, which a bare `useState` mirror of the same value would not.
 */
export function createExternalStore() {
  let listeners: Array<() => void> = [];

  return {
    subscribe(cb: () => void) {
      listeners.push(cb);
      return () => {
        listeners = listeners.filter((l) => l !== cb);
      };
    },
    notify() {
      for (const l of listeners) l();
    },
  };
}
