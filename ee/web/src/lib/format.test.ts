/**
 * The display side of the same two bugs. When `SearchPage` read `result.data` off a `SearchHit`,
 * it got `undefined`, and these helpers turned that into an empty cell -- the table rendered with
 * the right number of rows and columns and nothing in them, which is why it went unnoticed.
 *
 * These tests pin the shape of that failure: given a real payload the helpers produce visible
 * text, and given the `undefined` a mis-typed response produces, they must not silently emit
 * something that looks like content.
 */
import { describe, expect, it } from "vitest";
import { dataPreview, formatDate, formatDateTime, truncateId } from "./format";

describe("dataPreview", () => {
  it("renders a real data blob as visible text", () => {
    expect(dataPreview({ title: "Alpha note", body: "hello" })).toBe(
      '{"title":"Alpha note","body":"hello"}',
    );
  });

  it("truncates past the cutoff and marks it, at the caller's length", () => {
    const long = { body: "x".repeat(500) };
    // EntitiesPage uses the default (100); SearchPage passes 120.
    expect(dataPreview(long)).toHaveLength(101);
    expect(dataPreview(long, 120)).toHaveLength(121);
    expect(dataPreview(long).endsWith("…")).toBe(true);
  });

  it("returns empty -- not the string 'undefined' -- when handed nothing", () => {
    // This is what the mis-typed SearchPage actually passed in. An empty cell is bad; a cell
    // reading "undefined" would be worse, and this pins which one happens.
    const missing = undefined as unknown as Record<string, unknown>;
    expect(dataPreview(missing)).toBe("");
    expect(dataPreview(missing)).not.toContain("undefined");
  });
});

describe("truncateId", () => {
  it("shortens a UUID to its first 8 characters", () => {
    expect(truncateId("019fe064-7955-71e5-8e05-cdaec8620263")).toBe("019fe064");
  });

  it("leaves a shorter string alone", () => {
    expect(truncateId("abc")).toBe("abc");
  });
});

describe("date formatters", () => {
  // The two exist separately on purpose: list views render a bare date, detail views render the
  // time too. Collapsing them would silently change what half the pages display.
  it("formats date-only and date-time differently", () => {
    const iso = "2026-08-08T07:13:16Z";
    expect(formatDate(iso)).not.toEqual(formatDateTime(iso));
  });

  it("passes a malformed value through instead of rendering 'Invalid Date'", () => {
    expect(formatDate("not-a-date")).toBe("not-a-date");
    expect(formatDateTime("not-a-date")).toBe("not-a-date");
  });
});
