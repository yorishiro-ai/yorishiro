/**
 * `SchemaVersionDiff` diffs two serialized schema definitions. The whole point of serializing
 * through `stableJson` rather than `JSON.stringify` is that key order must not register as a
 * change -- otherwise every version diff is full of phantom moves.
 */
import { describe, expect, it } from "vitest";
import { stableJson } from "./stableJson";

describe("stableJson", () => {
  it("produces identical output for objects differing only in key order", () => {
    const a = { name: "note", fields: { title: "text", body: "text" } };
    const b = { fields: { body: "text", title: "text" }, name: "note" };
    expect(stableJson(a)).toBe(stableJson(b));
  });

  it("still reports a real difference", () => {
    const a = { name: "note", fields: { title: "text" } };
    const b = { name: "note", fields: { title: "number" } };
    expect(stableJson(a)).not.toBe(stableJson(b));
  });

  it("preserves array order, where position is meaningful", () => {
    expect(stableJson({ required: ["a", "b"] })).not.toBe(stableJson({ required: ["b", "a"] }));
  });

  it("sorts nested objects, not just the top level", () => {
    const a = { outer: { inner: { z: 1, a: 2 } } };
    const b = { outer: { inner: { a: 2, z: 1 } } };
    expect(stableJson(a)).toBe(stableJson(b));
  });
});
