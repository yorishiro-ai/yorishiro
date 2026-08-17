import type { FormEvent } from "react";
import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Plus, Search, X } from "lucide-react";
import { listEntities, listSchemas, searchEntities, getActiveSchema } from "@/lib/api";
import type { Entity, SchemaDetail } from "@/types/api";
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
import { formatDate, truncateId, dataPreview, entityLabel } from "@/lib/format";

const PAGE_SIZE = 50;

export function EntitiesPage() {
  const navigate = useNavigate();

  const [entities, setEntities] = useState<Entity[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(true);

  const [entityTypes, setEntityTypes] = useState<string[]>([]);
  const [typeFilter, setTypeFilter] = useState<string>("");

  const [searchQuery, setSearchQuery] = useState("");
  const [searchActive, setSearchActive] = useState(false);
  const [searching, setSearching] = useState(false);

  const fetchPage = useCallback(async (offset: number, entityType: string) => {
    const results = await listEntities({
      entity_type: entityType || undefined,
      offset,
      limit: PAGE_SIZE,
    });
    return results;
  }, []);

  const loadInitial = useCallback(
    async (entityType: string) => {
      setLoading(true);
      setError(null);
      setSearchActive(false);
      setSearchQuery("");
      try {
        const results = await fetchPage(0, entityType);
        setEntities(results);
        setHasMore(results.length === PAGE_SIZE);
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to load entities");
      } finally {
        setLoading(false);
      }
    },
    [fetchPage],
  );

  useEffect(() => {
    loadInitial(typeFilter);
  }, [typeFilter, loadInitial]);

  useEffect(() => {
    let cancelled = false;
    async function loadSchemas() {
      try {
        const schemaList = await listSchemas();
        const active = schemaList.filter((s) => s.status === "active");
        if (cancelled) return;

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

  async function handleLoadMore() {
    setLoadingMore(true);
    setError(null);
    try {
      const results = await fetchPage(entities.length, typeFilter);
      setEntities((prev) => [...prev, ...results]);
      setHasMore(results.length === PAGE_SIZE);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load more entities");
    } finally {
      setLoadingMore(false);
    }
  }

  async function handleSearchSubmit(event: FormEvent) {
    event.preventDefault();
    const query = searchQuery.trim();
    if (!query) {
      loadInitial(typeFilter);
      return;
    }
    setSearching(true);
    setError(null);
    try {
      const hits = await searchEntities(query, {
        entity_type: typeFilter || undefined,
      });
      setEntities(hits.map((hit) => hit.entity));
      setHasMore(false);
      setSearchActive(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Search failed");
    } finally {
      setSearching(false);
    }
  }

  function handleClearSearch() {
    setSearchQuery("");
    loadInitial(typeFilter);
  }

  const showLoadMore = !searchActive && hasMore && !loading && entities.length > 0;

  return (
    <div className="space-y-6 p-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Entities</h1>
          <p className="text-sm text-muted-foreground">
            Browse, search, and create entities across active schemas.
          </p>
        </div>
        <Button onClick={() => navigate("new")}>
          <Plus className="mr-2 h-4 w-4" />
          Create Entity
        </Button>
      </div>

      <Card>
        <CardHeader className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
          <CardTitle className="text-base font-medium">All Entities</CardTitle>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
            <form onSubmit={handleSearchSubmit} className="flex items-center gap-2">
              <div className="relative">
                <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="Search entities…"
                  className="w-56 pl-9"
                />
              </div>
              {searchActive && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={handleClearSearch}
                  aria-label="Clear search"
                >
                  <X className="h-4 w-4" />
                </Button>
              )}
              <Button type="submit" variant="secondary" size="sm" disabled={searching}>
                {searching ? "Searching…" : "Search"}
              </Button>
            </form>
            <select
              value={typeFilter}
              onChange={(e) => setTypeFilter(e.target.value)}
              className="h-10 rounded-md border border-input px-3 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
              aria-label="Filter by entity type"
            >
              <option value="">All types</option>
              {entityTypes.map((type) => (
                <option key={type} value={type}>
                  {type}
                </option>
              ))}
            </select>
          </div>
        </CardHeader>
        <CardContent>
          {error && (
            <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
              {error}
            </div>
          )}

          {loading ? (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          ) : entities.length === 0 ? (
            <div className="py-12 text-center text-sm text-muted-foreground">
              No entities found.
            </div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead>Data Preview</TableHead>
                    <TableHead>Created</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {entities.map((entity) => (
                    <TableRow
                      key={entity.id}
                      className="cursor-pointer"
                      onClick={() => navigate(`${entity.id}`)}
                    >
                      {/* The name, with the id beneath it. Ids are time-ordered, so a column
                          of id prefixes reads as the same value repeated on every row. */}
                      <TableCell>
                        <div className="font-medium">{entityLabel(entity)}</div>
                        <div className="font-mono text-xs text-muted-foreground">
                          {truncateId(entity.id)}
                        </div>
                      </TableCell>
                      <TableCell>
                        <Badge variant="secondary">{entity.entity_type}</Badge>
                      </TableCell>
                      <TableCell className="max-w-md truncate text-muted-foreground">
                        {dataPreview(entity.data)}
                      </TableCell>
                      <TableCell className="whitespace-nowrap text-muted-foreground">
                        {formatDate(entity.created_at)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}

          {showLoadMore && (
            <div className="mt-4 flex justify-center">
              <Button variant="secondary" onClick={handleLoadMore} disabled={loadingMore}>
                {loadingMore ? "Loading…" : "Load more"}
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
