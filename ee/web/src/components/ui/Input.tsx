import { cn } from "@/lib/cn";

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
  /**
   * Forwarded to the inner `<input>`. Declared explicitly because React 19 passes `ref` as an
   * ordinary prop to function components -- it would otherwise ride along in `...props` and work
   * by accident, with no type describing it.
   */
  ref?: React.Ref<HTMLInputElement>;
}

export function Input({ label, error, id, className, ...props }: InputProps) {
  const generatedId = id ?? props.name;

  return (
    <div className="w-full">
      {label && (
        <label htmlFor={generatedId} className="mb-1 block text-sm font-medium">
          {label}
        </label>
      )}
      <input
        id={generatedId}
        className={cn(
          "w-full rounded-md border border-input bg-card px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground",
          "focus:outline-none focus:ring-2 focus:ring-ring",
          "disabled:cursor-not-allowed disabled:opacity-50",
          error && "border-destructive focus:ring-destructive",
          className,
        )}
        aria-invalid={error ? true : undefined}
        aria-describedby={error && generatedId ? `${generatedId}-error` : undefined}
        {...props}
      />
      {error && (
        <p
          id={generatedId ? `${generatedId}-error` : undefined}
          className="mt-1 text-sm text-destructive"
        >
          {error}
        </p>
      )}
    </div>
  );
}
