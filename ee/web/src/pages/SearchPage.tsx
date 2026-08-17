import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Search as SearchIcon } from "lucide-react";
import { searchEntities, listSchemas, getActiveSchema } from "@/lib/api";
import type { Schema, SchemaDetail, SearchHit } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { Badge } from "@/components/ui/Badge";
import { Skeleton } from "@/components/ui/Skeleton";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/Table";
import { dataPreview, entityLabel } from "@/lib/format";

const DEFAULT_LIMIT = 25;

export function SearchPage() {
  const navigate = useNavigate();
  const { wsId } = useParams<{ wsId: string }>();

  const [entityTypes, setEntityTypes] = useState<string[]>([]);
  const [typeFilter, setTypeFilter] = useState<string>("");

  const [queryText, setQueryText] = useState("");
  const [results, setResults] = useState<SearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasSearched, setHasSearched] = useState(false);

  useEffect(() => {
    let cancelled = false;
    async function loadSchemas() {
      try {
        const schemaList: Schema[] = await listSchemas();
        const active = schemaList.filter((s) => s.status === "active");
        const details = await Promise.all(
          active.map((s) => getActiveSchema(s.name).catch(() => null as SchemaDetail | null)),
        );
        if (cancelled) return;
        const types = new Set<string>();
        for (const detail of details) {
          if (!detail) continue;
          for (const t of Object.keys(detail.definition.entity_types ?? {})) {
            types.add(t);
          }
        }
        setEntityTypes(Array.from(types).toSorted());
      } catch {
        // Non-fatal: filter dropdown just stays empty.
      }
    }
    loadSchemas();
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    const query = queryText.trim();
    if (!query) return;

    setSearching(true);
    setError(null);
    try {
      const found = await searchEntities(query, {
        entity_type: typeFilter || undefined,
        limit: DEFAULT_LIMIT,
      });
      setResults(found);
      setHasSearched(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Search failed");
      setResults([]);
      setHasSearched(true);
    } finally {
      setSearching(false);
    }
  }

  return (
    <div className="space-y-6 p-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Search</h1>
        <p className="text-sm text-muted-foreground">Search entities by text similarity.</p>
      </div>

      <Card>
        <CardHeader className="space-y-4">
          <CardTitle className="text-base font-medium">Query</CardTitle>
          <form onSubmit={handleSubmit} className="flex flex-col gap-3 sm:flex-row sm:items-center">
            <div className="relative flex-1">
              <SearchIcon className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={queryText}
                onChange={(e) => setQueryText(e.target.value)}
                placeholder="Search entities by text…"
                className="pl-9"
                aria-label="Search query"
              />
            </div>
            <select
              value={typeFilter}
              onChange={(e) => setTypeFilter(e.target.value)}
              className="h-10 rounded-md border border-input bg-card px-3 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
              aria-label="Filter by entity type"
            >
              <option value="">All types</option>
              {entityTypes.map((type) => (
                <option key={type} value={type}>
                  {type}
                </option>
              ))}
            </select>
            <Button type="submit" disabled={searching || !queryText.trim()}>
              {searching ? "Searching…" : "Search"}
            </Button>
          </form>
        </CardHeader>
        <CardContent>
          {error && (
            <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
              {error}
            </div>
          )}

          {searching ? (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          ) : !hasSearched ? (
            <div className="py-12 text-center text-sm text-muted-foreground">
              Enter a query above to search entities.
            </div>
          ) : results.length === 0 ? (
            <div className="py-12 text-center text-sm text-muted-foreground">No results found.</div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Entity Type</TableHead>
                    <TableHead>Data Preview</TableHead>
                    <TableHead className="text-right">Similarity</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {results.map(({ entity, distance }) => (
                    <TableRow
                      key={entity.id}
                      className="cursor-pointer"
                      onClick={() => navigate(`/ws/${wsId}/entities/${entity.id}`)}
                    >
                      <TableCell className="font-medium">{entityLabel(entity)}</TableCell>
                      <TableCell>
                        <Badge variant="secondary">{entity.entity_type}</Badge>
                      </TableCell>
                      <TableCell className="max-w-md truncate text-muted-foreground">
                        {dataPreview(entity.data, 120)}
                      </TableCell>
                      {/* The ordering is by distance, so showing it is what makes the ranking
                          legible. Rendered as similarity (1 - distance) because "higher is a
                          better match" is the direction a reader expects from a search result. */}
                      <TableCell className="whitespace-nowrap text-right font-mono text-xs text-muted-foreground">
                        {distance === null ? "—" : (1 - distance).toFixed(3)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
