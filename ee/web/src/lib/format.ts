/**
 * Display formatters shared across pages. Each was previously copy-pasted per page; the two
 * date variants are kept as separate names on purpose -- list views render a bare date, detail
 * views render date and time, and collapsing them into one `formatDate` would silently change
 * what half the pages display.
 *
 * All of them pass a malformed value straight through rather than rendering "Invalid Date": the
 * inputs are API timestamps, and showing the raw string makes a backend problem visible instead
 * of disguising it as a UI one.
 */

/** Date only ("Mar 3, 2026") -- for list/table columns where the time adds noise. */
export function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** Date and time, locale default -- for detail views where the exact moment matters. */
export function formatDateTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

/** First 8 characters of a UUID, the amount that stays readable in a table cell. */
export function truncateId(id: string): string {
  return id.slice(0, 8);
}

/**
 * One-line JSON preview of an entity's `data` blob for a table cell, ellipsised past `maxLength`.
 * Falls back to `String(data)` if the blob can't be stringified (a cycle, a `BigInt`), since a
 * preview column is never worth throwing over.
 */
export function dataPreview(data: Record<string, unknown>, maxLength = 100): string {
  let text: string;
  try {
    text = JSON.stringify(data);
  } catch {
    text = String(data);
  }
  if (!text) return "";
  return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}

/// Picks something human-readable to name an entity by.
///
/// The id prefix alone is not enough: ids are time-ordered, so entities created in the same
/// session share their leading characters and every one renders identically. Falls back to the
/// id only when the entity carries no string field to name it by.
export function entityLabel(
  entity: { id: string; data?: Record<string, unknown> },
  maxLength = 48,
): string {
  const named = ["title", "name", "label"]
    .map((k) => entity.data?.[k])
    .find((v) => typeof v === "string" && v.trim().length > 0) as string | undefined;
  const text = named ?? entity.id.slice(0, 8);
  return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}
