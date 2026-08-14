/**
 * Pins two properties of the workspace routes that a review flagged as broken.
 *
 * The guard route is pathless and its children carry absolute `/ws/:wsId/...` paths, which looks
 * as though the guard's own `useParams()` would see no `wsId` and redirect every workspace page
 * to the dashboard. React Router 7 propagates params to every matched level, so it does not --
 * and relative resolution works off the current URL rather than the parent's route pattern, so
 * the `import-export` redirect lands inside the workspace too.
 *
 * Both were measured rather than reasoned about. This file is what stops the "fix" from being
 * applied to code that is already correct.
 */
import { describe, expect, it } from "vitest";
import { matchRoutes, resolvePath } from "react-router-dom";

describe("workspace routing under a pathless guard", () => {
  it("gives the child's :wsId to every matched level, including the pathless guard", () => {
    const routes = [{ path: "/", children: [{ children: [{ path: "/ws/:wsId/schema" }] }] }];
    const matches = matchRoutes(routes, "/ws/abc123/schema")!;
    for (const m of matches) expect(m.params).toEqual({ wsId: "abc123" });
  });

  it("resolves the import-export redirect to the workspace's own schema/io", () => {
    // The real route is `/ws/:wsId/import-export` redirecting to `../schema/io`.
    // Relative resolution is against the *current path*, not the parent route pattern.
    const resolved = resolvePath("../schema/io", "/ws/abc123/import-export");
    expect(resolved.pathname).toBe("/ws/abc123/schema/io");
  });
});
