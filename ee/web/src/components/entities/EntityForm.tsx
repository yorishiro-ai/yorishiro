import { useMemo, useState } from "react";
import { Plus, X } from "lucide-react";
import type { EntityTypeDef, FieldDef } from "@/types/api";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { FormJsonToggle } from "@/components/ui/FormJsonToggle";

interface EntityFormProps {
  entityTypeDef: EntityTypeDef;
  onSubmit: (data: Record<string, unknown>) => void;
  submitting: boolean;
  initialData?: Record<string, unknown>;
  submitLabel?: string;
  onCancel?: () => void;
  defaultJsonMode?: boolean;
}

export function EntityForm({
  entityTypeDef,
  onSubmit,
  submitting,
  initialData,
  submitLabel,
  onCancel,
  defaultJsonMode,
}: EntityFormProps) {
  const fields = entityTypeDef.fields;
  const fieldEntries = useMemo(() => {
    const entries = Object.entries(fields);
    entries.sort(([, a], [, b]) => {
      if (a.required && !b.required) return -1;
      if (!a.required && b.required) return 1;
      return 0;
    });
    return entries;
  }, [fields]);

  const initialValues = useMemo(() => {
    if (initialData) return { ...initialData };
    const values: Record<string, unknown> = {};
    for (const [name, def] of fieldEntries) {
      if (def.default !== undefined) {
        values[name] = def.default;
      } else if (def.type === "boolean") {
        values[name] = false;
      } else if (def.type === "array") {
        values[name] = [];
      }
    }
    return values;
  }, [fieldEntries, initialData]);

  function handleSubmit(raw: Record<string, unknown>) {
    const data: Record<string, unknown> = {};
    for (const [name, def] of fieldEntries) {
      const val = raw[name];
      if (val === undefined || val === "" || (Array.isArray(val) && val.length === 0)) {
        if (def.required) {
          data[name] = def.type === "array" ? [] : "";
        }
        continue;
      }
      if (def.type === "integer" || def.type === "number") {
        const num = Number(val);
        if (!Number.isNaN(num)) data[name] = num;
      } else {
        data[name] = val;
      }
    }
    onSubmit(data);
  }

  return (
    <FormJsonToggle
      data={initialValues}
      onSubmit={handleSubmit}
      submitting={submitting}
      submitLabel={submitLabel ?? "Create Entity"}
      onCancel={onCancel}
      defaultJsonMode={defaultJsonMode}
    >
      {({ values, setField }) => (
        <>
          {fieldEntries.map(([name, def]) => (
            <FieldInput key={name} name={name} def={def} value={values[name]} onChange={setField} />
          ))}
        </>
      )}
    </FormJsonToggle>
  );
}

interface FieldInputProps {
  name: string;
  def: FieldDef;
  value: unknown;
  onChange: (name: string, value: unknown) => void;
}

function FieldInput({ name, def, value, onChange }: FieldInputProps) {
  const label = name.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  const id = `field-${name}`;

  if (def.enum) {
    return (
      <FieldWrapper id={id} label={label} def={def}>
        <select
          id={id}
          value={(value as string) ?? ""}
          onChange={(e) => onChange(name, e.target.value)}
          className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
        >
          {!def.required && <option value="">-- Select --</option>}
          {def.enum.map((opt) => (
            <option key={opt} value={opt}>
              {opt}
            </option>
          ))}
        </select>
      </FieldWrapper>
    );
  }

  if (def.type === "boolean") {
    return (
      <div className="flex items-center gap-3 py-1">
        <input
          id={id}
          type="checkbox"
          checked={Boolean(value)}
          onChange={(e) => onChange(name, e.target.checked)}
          className="h-4 w-4 rounded border-input text-primary focus:ring-ring"
        />
        <label htmlFor={id} className="text-sm font-medium text-foreground">
          {label}
          {def.description && (
            <span className="ml-2 font-normal text-muted-foreground">{def.description}</span>
          )}
        </label>
      </div>
    );
  }

  if (def.type === "array" && def.items?.type === "string") {
    return (
      <FieldWrapper id={id} label={label} def={def}>
        <TagInput
          id={id}
          value={(value as string[]) ?? []}
          onChange={(tags) => onChange(name, tags)}
        />
      </FieldWrapper>
    );
  }

  if (def.type === "string" && def["x-ui"]?.widget === "textarea") {
    return (
      <FieldWrapper id={id} label={label} def={def}>
        <textarea
          id={id}
          value={(value as string) ?? ""}
          onChange={(e) => onChange(name, e.target.value)}
          rows={4}
          maxLength={def.maxLength}
          className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
        />
      </FieldWrapper>
    );
  }

  if (def.type === "string") {
    let inputType = "text";
    if (def.format === "date") inputType = "date";
    else if (def.format === "uri" || def.format === "url") inputType = "url";
    else if (def.format === "email") inputType = "email";

    return (
      <FieldWrapper id={id} label={label} def={def}>
        <Input
          id={id}
          type={inputType}
          value={(value as string) ?? ""}
          onChange={(e) => onChange(name, e.target.value)}
          maxLength={def.maxLength}
          required={def.required}
          placeholder={def.description ?? ""}
        />
      </FieldWrapper>
    );
  }

  if (def.type === "integer" || def.type === "number") {
    return (
      <FieldWrapper id={id} label={label} def={def}>
        <Input
          id={id}
          type="number"
          // A number field can still be handed an object by a schema change or a hand-edited
          // entity, and `String({})` renders "[object Object]" into the input. Anything that is
          // not a number or string is treated as no value at all.
          value={typeof value === "number" || typeof value === "string" ? String(value) : ""}
          onChange={(e) => onChange(name, e.target.value)}
          step={def.type === "integer" ? "1" : "any"}
          required={def.required}
        />
      </FieldWrapper>
    );
  }

  return (
    <FieldWrapper id={id} label={label} def={def}>
      <textarea
        id={id}
        value={typeof value === "string" ? value : JSON.stringify(value ?? "", null, 2)}
        onChange={(e) => {
          try {
            onChange(name, JSON.parse(e.target.value));
          } catch {
            onChange(name, e.target.value);
          }
        }}
        rows={3}
        spellCheck={false}
        className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
        placeholder="JSON value"
      />
    </FieldWrapper>
  );
}

function FieldWrapper({
  id,
  label,
  def,
  children,
}: {
  id: string;
  label: string;
  def: FieldDef;
  children: React.ReactNode;
}) {
  return (
    <div>
      <label
        htmlFor={id}
        className="mb-1 flex items-center gap-1.5 text-sm font-medium text-foreground"
      >
        {label}
        {def.required && <span className="text-destructive">*</span>}
        {def["x-embed"] && (
          <span className="rounded bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-link">
            embed
          </span>
        )}
      </label>
      {children}
      {def.description && <p className="mt-1 text-xs text-muted-foreground">{def.description}</p>}
    </div>
  );
}

interface TagInputProps {
  id: string;
  value: string[];
  onChange: (tags: string[]) => void;
}

function TagInput({ id, value, onChange }: TagInputProps) {
  const [draft, setDraft] = useState("");

  function addTag() {
    const tag = draft.trim();
    if (tag && !value.includes(tag)) {
      onChange([...value, tag]);
    }
    setDraft("");
  }

  return (
    <div>
      <div className="flex flex-wrap gap-1.5 mb-2">
        {value.map((tag) => (
          <span
            key={tag}
            className="inline-flex items-center gap-1 rounded-full bg-secondary px-2.5 py-0.5 text-xs font-medium text-secondary-foreground"
          >
            {tag}
            <button
              type="button"
              onClick={() => onChange(value.filter((t) => t !== tag))}
              className="ml-0.5 text-muted-foreground hover:text-foreground"
              aria-label={`Remove ${tag}`}
            >
              <X className="h-3 w-3" />
            </button>
          </span>
        ))}
      </div>
      <div className="flex gap-2">
        <Input
          id={id}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              addTag();
            }
          }}
          placeholder="Add tag and press Enter"
          className="flex-1"
        />
        <Button type="button" variant="secondary" size="sm" onClick={addTag}>
          <Plus className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}
