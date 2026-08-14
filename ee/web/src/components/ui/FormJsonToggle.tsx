import type { ReactNode } from "react";
import { useState } from "react";
import { Code, FormInput } from "lucide-react";
import { Button } from "./Button";

interface FormJsonToggleProps {
  data: Record<string, unknown>;
  onSubmit: (data: Record<string, unknown>) => void;
  submitting: boolean;
  submitLabel?: string;
  onCancel?: () => void;
  defaultJsonMode?: boolean;
  children: (props: {
    values: Record<string, unknown>;
    setField: (name: string, value: unknown) => void;
  }) => ReactNode;
}

export function FormJsonToggle({
  data,
  onSubmit,
  submitting,
  submitLabel = "Save",
  onCancel,
  defaultJsonMode = false,
  children,
}: FormJsonToggleProps) {
  const [values, setValues] = useState<Record<string, unknown>>({ ...data });
  const [jsonMode, setJsonMode] = useState(defaultJsonMode);
  const [jsonDraft, setJsonDraft] = useState(defaultJsonMode ? JSON.stringify(data, null, 2) : "");
  const [jsonError, setJsonError] = useState<string | null>(null);

  function setField(name: string, value: unknown) {
    setValues((prev) => ({ ...prev, [name]: value }));
  }

  function switchToJson() {
    setJsonDraft(JSON.stringify(values, null, 2));
    setJsonError(null);
    setJsonMode(true);
  }

  function switchToForm() {
    try {
      const parsed = JSON.parse(jsonDraft);
      if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
        setValues(parsed);
      }
    } catch {
      // keep current values
    }
    setJsonMode(false);
  }

  function handleSubmit(e?: React.FormEvent) {
    e?.preventDefault();
    if (jsonMode) {
      try {
        const parsed = JSON.parse(jsonDraft);
        if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
          setJsonError("Data must be a JSON object.");
          return;
        }
        onSubmit(parsed);
      } catch {
        setJsonError("Invalid JSON.");
      }
      return;
    }
    onSubmit(values);
  }

  const actionButtons = (
    <div className="flex justify-end gap-2 pt-2">
      {onCancel && (
        <Button type="button" variant="secondary" onClick={onCancel} disabled={submitting}>
          Cancel
        </Button>
      )}
      <Button
        type={jsonMode ? "button" : "submit"}
        onClick={jsonMode ? handleSubmit : undefined}
        disabled={submitting}
      >
        {submitting ? "Saving..." : submitLabel}
      </Button>
    </div>
  );

  if (jsonMode) {
    return (
      <div className="space-y-4">
        <div className="flex items-center justify-between">
          <span className="text-sm font-medium text-muted-foreground">JSON Mode</span>
          <Button type="button" variant="ghost" size="sm" onClick={switchToForm}>
            <FormInput className="mr-1 h-4 w-4" />
            Form Mode
          </Button>
        </div>
        {jsonError && (
          <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {jsonError}
          </div>
        )}
        <textarea
          value={jsonDraft}
          onChange={(e) => {
            setJsonDraft(e.target.value);
            setJsonError(null);
          }}
          rows={16}
          spellCheck={false}
          className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
        />
        {actionButtons}
      </div>
    );
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      <div className="flex items-center justify-end">
        <Button type="button" variant="ghost" size="sm" onClick={switchToJson}>
          <Code className="mr-1 h-4 w-4" />
          JSON Mode
        </Button>
      </div>
      {children({ values, setField })}
      {actionButtons}
    </form>
  );
}
