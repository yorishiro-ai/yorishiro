/**
 * Serializes a value to JSON with object keys sorted, for diffing.
 *
 * Key order in a JSON object is not significant, and `create_schema` round-trips the definition
 * through `serde_json::Value` without preserving it. Diffing the raw serialization would report
 * a re-ordered but otherwise identical definition as a change; sorting first means the diff
 * shows only what actually differs.
 *
 * Arrays keep their order -- there, position *is* meaningful.
 */
export function stableJson(value: unknown): string {
  return `${JSON.stringify(
    value,
    (_key, val) => {
      if (val && typeof val === "object" && !Array.isArray(val)) {
        return Object.fromEntries(
          Object.entries(val as Record<string, unknown>).toSorted(([a], [b]) => a.localeCompare(b)),
        );
      }
      return val;
    },
    2,
  )}\n`;
}
