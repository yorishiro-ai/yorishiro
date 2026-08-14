/** Thresholds are ordered ascending; the last one whose `at` the ratio reaches wins. */
export type Threshold = { at: number; className: string };

export const USAGE_THRESHOLDS: Threshold[] = [
  { at: 0, className: "text-primary" },
  { at: 0.75, className: "text-amber-500" },
  { at: 0.9, className: "text-destructive" },
];

/**
 * Picks the colour for a usage ratio (`value / limit`).
 *
 * Ratios above 1 keep the highest threshold rather than falling off the end of the list -- an
 * over-quota workspace must not render in the same colour as an empty one.
 */
export function thresholdClass(ratio: number, thresholds: Threshold[] = USAGE_THRESHOLDS): string {
  let cls = thresholds[0]?.className ?? "";
  for (const t of thresholds) {
    if (ratio >= t.at) cls = t.className;
  }
  return cls;
}
