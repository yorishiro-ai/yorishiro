import { Badge } from "@/components/ui/Badge";

/**
 * A schema's status as a badge: `active` is emphasised, anything else is muted.
 *
 * Deliberately not merged with `SchemasPage`'s own `statusVariant`, which maps four statuses
 * case-insensitively (`draft` and `deprecated` get their own variants). That mapping is richer,
 * and adopting it here would change what the schema detail views render for those statuses.
 */
export function StatusBadge({ status }: { status: string }) {
  const variant = status === "active" ? "default" : "secondary";
  return <Badge variant={variant}>{status}</Badge>;
}
