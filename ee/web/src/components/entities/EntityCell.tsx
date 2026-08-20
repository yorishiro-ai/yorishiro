import type { Entity, FieldDef } from "@/types/api";
import { Badge } from "@/components/ui/Badge";
import { formatDate, truncateId, entityLabel } from "@/lib/format";
import { BUILT_IN_LABELS } from "./ColumnPicker";

/// The header text for a column, whether it is a schema field or one of the built-ins.
export function columnHeader(name: string): string {
  return BUILT_IN_LABELS[name] ?? name;
}

/// One value, rendered for a table cell.
///
/// A field the schema no longer defines is stored in the preference and reaches here anyway, so
/// an unknown name renders as an em dash rather than throwing: cleaning the preference up on
/// write would make a schema migration responsible for display settings.
export function EntityCell({
  entity,
  column,
  def,
}: {
  entity: Entity;
  column: string;
  def?: FieldDef;
}) {
  if (column === "__label") {
    return (
      <>
        <div className="font-medium">{entityLabel(entity)}</div>
        {/* The id beneath the name. Ids are time-ordered, so a column of id prefixes reads as
            the same value repeated on every row. */}
        <div className="font-mono text-xs text-muted-foreground">{truncateId(entity.id)}</div>
      </>
    );
  }
  if (column === "__type") {
    return <Badge variant="secondary">{entity.entity_type}</Badge>;
  }
  if (column === "__created") {
    return <span className="whitespace-nowrap">{formatDate(entity.created_at)}</span>;
  }

  const value = entity.data?.[column];
  if (value === undefined || value === null) {
    return <span className="text-muted-foreground">—</span>;
  }
  if (typeof value === "boolean") {
    // A bare "false" reads as missing data next to an empty cell; the words do not.
    return <span>{value ? "Yes" : "No"}</span>;
  }
  if (def?.type === "array" || def?.type === "object" || typeof value === "object") {
    return <span className="font-mono text-xs text-muted-foreground">{JSON.stringify(value)}</span>;
  }
  // `value` is `unknown`, so this is the last branch rather than a safe default: anything that
  // is not a primitive has already gone to JSON above, and `String({})` would render
  // "[object Object]" into a cell.
  if (typeof value === "string" || typeof value === "number") {
    return <span>{String(value)}</span>;
  }
  return <span className="font-mono text-xs text-muted-foreground">{JSON.stringify(value)}</span>;
}
