/**
 * Every failed request in the SPA renders whatever this returns. Two error shapes reach it --
 * the community API's top-level `{message}` and the hosted API's nested `{error: {message}}` --
 * and reading `json.error` alone yields the *object* for the latter, which reaches the screen as
 * "[object Object]" in place of the reason. That is what the marketplace's fork button showed
 * for a legitimate 409.
 */
import { describe, expect, it } from "vitest";
import { errorMessage } from "./errorMessage";

describe("errorMessage", () => {
  it("reads the community API's top-level message", () => {
    expect(errorMessage('{"message":"schema not found"}', "Bad Request")).toBe("schema not found");
  });

  it("reads the hosted API's nested message", () => {
    expect(errorMessage('{"error":{"message":"already exists"}}', "Conflict")).toBe(
      "already exists",
    );
  });

  it("never returns a stringified object", () => {
    const result = errorMessage('{"error":{"code":42}}', "Conflict");
    expect(result).not.toContain("[object Object]");
  });

  it("falls back to a plain-text body", () => {
    expect(errorMessage("upstream timed out", "Bad Gateway")).toBe("upstream timed out");
  });

  it("falls back to the status text for an empty body", () => {
    expect(errorMessage("", "Service Unavailable")).toBe("Service Unavailable");
  });

  it("prefers a string error over the fallback", () => {
    expect(errorMessage('{"error":"nope"}', "Bad Request")).toBe("nope");
  });
});
