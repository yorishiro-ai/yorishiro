import { useEffect, useMemo, useState } from "react";
import { parseDiffFromFile } from "@pierre/diffs";
import { FileDiff } from "@pierre/diffs/react";
import { getSchemaById } from "@/lib/api";
import { stableJson } from "@/lib/stableJson";
import type { Schema, SchemaDetail } from "@/types/api";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/Card";

/**
 * Git-style diff between two versions of a schema definition.
 *
 * Versions are passed in already-listed (the caller's version switcher loads them), but their
 * definitions are not: `listSchemas` returns summaries without `definition`, so both sides are
 * fetched by id here.
 */
export function SchemaVersionDiff({ versions }: { versions: Schema[] }) {
  const sorted = useMemo(() => versions.toSorted((a, b) => b.version - a.version), [versions]);

  const [newId, setNewId] = useState<string>(sorted[0]?.id ?? "");
  const [oldId, setOldId] = useState<string>(sorted[1]?.id ?? sorted[0]?.id ?? "");
  const [pair, setPair] = useState<{ old: SchemaDetail; new: SchemaDetail } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!oldId || !newId) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    Promise.all([getSchemaById(oldId), getSchemaById(newId)])
      .then(([oldSchema, newSchema]) => {
        if (!cancelled) setPair({ old: oldSchema, new: newSchema });
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load versions");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [oldId, newId]);

  const fileDiff = useMemo(() => {
    if (!pair) return null;
    return parseDiffFromFile(
      {
        name: `${pair.old.name}.v${pair.old.version}.json`,
        contents: stableJson(pair.old.definition),
        cacheKey: pair.old.id,
      },
      {
        name: `${pair.new.name}.v${pair.new.version}.json`,
        contents: stableJson(pair.new.definition),
        cacheKey: pair.new.id,
      },
    );
  }, [pair]);

  if (sorted.length < 2) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Version Diff</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">
            Only one version exists. A diff appears once this schema has been edited at least once.
          </p>
        </CardContent>
      </Card>
    );
  }

  const unchanged = fileDiff !== null && fileDiff.hunks.length === 0;

  return (
    <Card>
      <CardHeader>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <CardTitle className="text-lg">Version Diff</CardTitle>
          <div className="flex items-center gap-2">
            <label htmlFor="diff-old-version" className="text-xs font-medium text-muted-foreground">
              From
            </label>
            <select
              id="diff-old-version"
              value={oldId}
              onChange={(e) => setOldId(e.target.value)}
              disabled={loading}
              className="h-9 rounded-md border border-input bg-card px-3 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring disabled:opacity-60"
            >
              {sorted.map((v) => (
                <option key={v.id} value={v.id}>
                  v{v.version}
                </option>
              ))}
            </select>
            <label htmlFor="diff-new-version" className="text-xs font-medium text-muted-foreground">
              To
            </label>
            <select
              id="diff-new-version"
              value={newId}
              onChange={(e) => setNewId(e.target.value)}
              disabled={loading}
              className="h-9 rounded-md border border-input bg-card px-3 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring disabled:opacity-60"
            >
              {sorted.map((v) => (
                <option key={v.id} value={v.id}>
                  v{v.version}
                </option>
              ))}
            </select>
          </div>
        </div>
        <CardDescription>Changes to the schema definition between two versions.</CardDescription>
      </CardHeader>
      <CardContent>
        {error && <p className="text-sm text-destructive">{error}</p>}
        {loading && <p className="text-sm text-muted-foreground">Loading versions...</p>}
        {!loading && !error && unchanged && (
          <p className="text-sm text-muted-foreground">
            These two versions have identical definitions.
          </p>
        )}
        {!loading && !error && fileDiff && !unchanged && (
          <div className="overflow-x-auto">
            <FileDiff fileDiff={fileDiff} />
          </div>
        )}
      </CardContent>
    </Card>
  );
}
