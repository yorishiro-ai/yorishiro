import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { thresholdClass } from "./thresholds";

/**
 * Grafana-style dashboard primitives: a titled panel, a big-number stat, and a usage bar.
 *
 * These deliberately do not reuse `Card`. A Grafana panel is denser than a card -- a small
 * uppercase title bar, no description line, and content that runs to the panel edge -- and
 * mixing the two on one page reads as two different designs rather than one dashboard.
 */
export function Panel({
  title,
  actions,
  className,
  children,
}: {
  title: string;
  actions?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex flex-col rounded-md border border-border bg-card shadow-sm",
        "transition-colors hover:border-primary/40",
        className,
      )}
    >
      <div className="flex items-center justify-between border-b border-border px-3 py-2">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          {title}
        </h3>
        {actions}
      </div>
      <div className="flex-1 p-3">{children}</div>
    </div>
  );
}

/**
 * A single big number, optionally coloured by how close it sits to a limit.
 *
 * `limit` of `null` means unlimited -- the number is shown plainly with no threshold colour,
 * since there is nothing to be close to.
 */
export function Stat({
  value,
  unit,
  limit,
  caption,
}: {
  value: number | string;
  unit?: string;
  limit?: number | null;
  caption?: string;
}) {
  const numeric = typeof value === "number" ? value : null;
  const ratio = numeric !== null && limit ? numeric / limit : 0;
  const colour = limit ? thresholdClass(ratio) : "text-foreground";

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline gap-1.5">
        <span className={cn("text-3xl font-semibold leading-none tabular-nums", colour)}>
          {typeof value === "number" ? value.toLocaleString() : value}
        </span>
        {unit && <span className="text-sm text-muted-foreground">{unit}</span>}
      </div>
      {caption && <p className="text-xs text-muted-foreground">{caption}</p>}
    </div>
  );
}

/**
 * Horizontal usage bar against a quota. Renders nothing when `limit` is null -- an unlimited
 * quota has no bar to fill, and a full-width bar would read as "at capacity".
 */
export function UsageBar({ value, limit }: { value: number; limit: number | null }) {
  if (limit === null || limit <= 0) return null;
  const ratio = Math.min(value / limit, 1);
  const pct = Math.round(ratio * 100);

  return (
    <div className="mt-3 space-y-1">
      <div
        className="h-1.5 w-full overflow-hidden rounded-full bg-secondary"
        role="progressbar"
        aria-valuenow={value}
        aria-valuemin={0}
        aria-valuemax={limit}
        aria-label={`${value} of ${limit} used`}
      >
        <div
          className={cn(
            "h-full rounded-full transition-all",
            thresholdClass(ratio).replace("text-", "bg-"),
          )}
          style={{ width: `${pct}%` }}
        />
      </div>
      <p className="text-xs text-muted-foreground tabular-nums">
        {pct}% of {limit.toLocaleString()}
      </p>
    </div>
  );
}
