/**
 * The query strings the paged endpoints send.
 *
 * Both of these read as working when they are not: `GET /api/marketplace` and `GET /api/entities`
 * default to 50 server-side, so a client that sends no `limit`/`offset` gets a first page and no
 * indication that anything is missing. `listMarketplace` sent neither for as long as the endpoint
 * has accepted them, and the page had no way to reach template 51.
 *
 * So what is pinned here is the URL, not the response: a wrong or absent parameter is invisible in
 * the rendered result and visible only in what was asked for.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listEntities, listMarketplace } from "./api";

/** The paths `fetch` was called with, in order. */
let requested: string[];

beforeEach(() => {
  requested = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      // `RequestInfo` includes `Request`, whose `String()` is "[object Object]". Every caller
      // here passes a string, but reading `.url` for the object case keeps a future one from
      // recording a path that is not a path.
      requested.push(
        typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
      );
      return Promise.resolve(
        new Response("[]", { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("marketplace paging", () => {
  it("asks for a bounded page rather than everything", async () => {
    await listMarketplace({ limit: 50 });

    expect(requested[0]).toContain("limit=50");
  });

  it("advances by offset, which is how the second page is reachable at all", async () => {
    await listMarketplace({ offset: 50, limit: 50 });

    const url = new URL(requested[0], "http://localhost");
    expect(url.searchParams.get("offset")).toBe("50");
    expect(url.searchParams.get("limit")).toBe("50");
  });

  it("sends no query at all when it wants the server's own default", async () => {
    await listMarketplace();

    // Not `?offset=0&limit=`: an explicit empty value is a different request from an absent one,
    // and the server parses what it is given.
    expect(requested[0]).toBe("/api/marketplace");
  });
});

describe("entity filtering", () => {
  it("sends the filter as JSON, since the server parses it as a containment document", async () => {
    await listEntities({ entity_type: "task", filter: { done: true } });

    const url = new URL(requested[0], "http://localhost");
    expect(url.searchParams.get("filter")).toBe('{"done":true}');
    expect(url.searchParams.get("entity_type")).toBe("task");
  });

  it("keeps a boolean a boolean", async () => {
    await listEntities({ filter: { done: false } });

    // `{"done":"false"}` would be a string and `data @> filter` would match nothing, which looks
    // identical to "no rows are done" in the table.
    expect(new URL(requested[0], "http://localhost").searchParams.get("filter")).toBe(
      '{"done":false}',
    );
  });

  it("omits an empty filter rather than sending an empty document", async () => {
    await listEntities({ entity_type: "task", filter: {} });

    // `filter={}` matches everything, so it is harmless but pointless; more importantly it would
    // make an unfiltered request indistinguishable from a filtered one in a log.
    expect(new URL(requested[0], "http://localhost").searchParams.has("filter")).toBe(false);
  });
});
