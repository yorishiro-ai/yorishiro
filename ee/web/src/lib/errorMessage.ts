/**
 * Extracts a human-readable reason from an error response body.
 *
 * Two shapes are in play: the community API answers `{message}` at the top level, and the hosted
 * API nests it as `{error: {message}}`. Reading `json.error` alone yields the *object* for the
 * latter, which renders as "[object Object]" where the reason should be -- so the nested form is
 * checked first, and anything that still is not a string falls back to the raw body.
 */
export function errorMessage(body: string, fallback: string): string {
  try {
    const json = JSON.parse(body);
    const message = json.message ?? json.error?.message ?? json.error;
    if (typeof message === "string" && message) return message;
  } catch {
    // Not JSON -- the raw body is the best available reason.
  }
  return body || fallback;
}
