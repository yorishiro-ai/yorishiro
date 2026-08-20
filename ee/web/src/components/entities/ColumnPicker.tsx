import { useEffect, useState } from "react";
import { Columns3 } from "lucide-react";
import type { FieldDef } from "@/types/api";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";

/// Columns every entity has, whatever its schema says.
///
/// They are not schema fields, so they cannot be discovered from `entity_types`, but a table
/// without at least a name is a list of blank rows. Offered alongside the schema's own fields so
/// one checkbox list covers the whole table rather than two lists with different rules.
export const BUILT_IN_COLUMNS = ["__label", "__type", "__created"] as const;

export const BUILT_IN_LABELS: Record<string, string> = {
  __label: "Name",
  __type: "Type",
  __created: "Created",
};

/// What the table shows for a workspace that has never chosen.
///
/// The previous fixed layout, plus the first two schema fields so a new workspace sees its own
/// data rather than a JSON blob. Two, because a wider default would push the built-in columns off
/// a laptop screen before anyone has chosen anything.
export function defaultColumns(fields: Record<string, FieldDef>): string[] {
  const scalar = Object.entries(fields)
    .filter(([, def]) => def.type !== "object" && def.type !== "array")
    .map(([name]) => name);
  return ["__label", "__type", ...scalar.slice(0, 2), "__created"];
}

export interface ColumnPickerProps {
  open: boolean;
  onClose: () => void;
  entityType: string;
  fields: Record<string, FieldDef>;
  selected: string[];
  maxColumns: number;
  saving: boolean;
  onSave: (columns: string[]) => void;
  onReset: () => void;
}

/// Which columns the table shows, as checkboxes over everything available.
///
/// Local state until Save, so a half-made selection is not written and an unchecked box can be
/// re-checked without a round trip.
export function ColumnPicker({
  open,
  onClose,
  entityType,
  fields,
  selected,
  maxColumns,
  saving,
  onSave,
  onReset,
}: ColumnPickerProps) {
  const [draft, setDraft] = useState<string[]>(selected);

  // Reopening after a save elsewhere must show what is stored, not what was drafted last time.
  useEffect(() => {
    if (open) setDraft(selected);
  }, [open, selected]);

  const available = [...BUILT_IN_COLUMNS, ...Object.keys(fields)];
  const atLimit = draft.length >= maxColumns;

  function toggle(name: string) {
    setDraft((prev) => (prev.includes(name) ? prev.filter((c) => c !== name) : [...prev, name]));
  }

  return (
    <Dialog open={open} onClose={onClose} title={`Columns for ${entityType}`} className="max-w-md">
      <p className="text-sm text-muted-foreground">
        Checked columns appear in the table, in the order you check them.
      </p>

      <div className="mt-4 max-h-80 space-y-1 overflow-y-auto">
        {available.map((name) => {
          const checked = draft.includes(name);
          const label = BUILT_IN_LABELS[name] ?? name;
          const def = fields[name];
          return (
            <label
              key={name}
              className="flex cursor-pointer items-center gap-3 rounded-md px-2 py-1.5 hover:bg-accent"
            >
              <input
                type="checkbox"
                checked={checked}
                // Unchecking must stay possible at the limit, or the only way out is Reset.
                disabled={!checked && atLimit}
                onChange={() => toggle(name)}
                className="h-4 w-4 accent-primary"
              />
              <span className="flex-1 text-sm">{label}</span>
              {def && <span className="font-mono text-xs text-muted-foreground">{def.type}</span>}
              {checked && (
                <span className="text-xs tabular-nums text-muted-foreground">
                  {draft.indexOf(name) + 1}
                </span>
              )}
            </label>
          );
        })}
      </div>

      <p className="mt-3 text-xs text-muted-foreground">
        {draft.length} of {maxColumns} columns selected.
      </p>

      <div className="mt-4 flex justify-between gap-2">
        <Button variant="ghost" size="sm" onClick={onReset} disabled={saving}>
          Reset to default
        </Button>
        <div className="flex gap-2">
          <Button variant="secondary" size="sm" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => onSave(draft)} disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

/// The button that opens the picker.
export function ColumnPickerButton({ onClick }: { onClick: () => void }) {
  return (
    <Button variant="secondary" size="sm" onClick={onClick} aria-label="Choose columns">
      <Columns3 className="mr-2 h-4 w-4" />
      Columns
    </Button>
  );
}
