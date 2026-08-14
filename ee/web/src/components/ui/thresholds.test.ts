/**
 * The usage colour is the only thing on the dashboard that tells an operator they are about to
 * hit a quota. Getting the boundaries wrong means either crying wolf at 10% or staying calm at
 * 95%, so the thresholds are pinned here rather than eyeballed in a browser.
 */
import { describe, expect, it } from "vitest";
import { thresholdClass } from "./thresholds";

describe("thresholdClass", () => {
  it("stays neutral well under the limit", () => {
    expect(thresholdClass(0)).toBe("text-primary");
    expect(thresholdClass(0.5)).toBe("text-primary");
  });

  it("warns from three quarters", () => {
    expect(thresholdClass(0.75)).toBe("text-amber-500");
    expect(thresholdClass(0.89)).toBe("text-amber-500");
  });

  it("goes critical from ninety percent", () => {
    expect(thresholdClass(0.9)).toBe("text-destructive");
    expect(thresholdClass(1)).toBe("text-destructive");
  });

  it("keeps the critical colour past the limit rather than wrapping around", () => {
    // An over-quota ratio must not fall off the end of the list and read as healthy.
    expect(thresholdClass(2.5)).toBe("text-destructive");
  });
});
