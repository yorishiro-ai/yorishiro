import type { FormEvent } from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Plus, Search, X } from "lucide-react";
import {
  listEntities,
  listSchemas,
  searchEntities,
  getActiveSchema,
  listColumnPreferences,
  setColumnPreference,
  resetColumnPreference,
} from "@/lib/api";
import type { Entity, FieldDef, SchemaDetail } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { Skeleton } from "@/components/ui/Skeleton";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/Table";
import {
  ColumnPicker,
  ColumnPickerButton,
  defaultColumns,
} from "@/components/entities/ColumnPicker";
import { EntityCell, columnHeader } from "@/components/entities/EntityCell";

const PAGE_SIZE = 50;

/// Mirrors `MAX_VISIBLE_COLUMNS` in `ee/crates/yorishiro-hosted/src/models/entity_columns.rs`.
/// The server refuses more than this; the picker stops before asking so the refusal is not the
/// first the reader hears of the limit.
const MAX_COLUMNS = 12;

export function EntitiesPage() {
  const navigate = useNavigate();

  const [entities, setEntities] = useState<Entity[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(true);

  const [entityTypes, setEntityTypes] = useState<string[]>([]);
  const [typeFilter, setTypeFilter] = useState<string>("");
  /// Every active schema's fields, keyed by entity type, so the table and the picker can both
  /// ask what a type is made of without another round trip.
  const [fieldsByType, setFieldsByType] = useState<Record<string, Record<string, FieldDef>>>({});

  const [searchQuery, setSearchQuery] = useState("");
  const [searchActive, setSearchActive] = useState(false);
  const [searching, setSearching] = useState(false);

  /// Field-level filters, as exact values. The server matches with JSONB containment, so this
  /// cannot express a range or a substring; the input is only offered for fields where equality
  /// is the natural question (enums and booleans).
  const [fieldFilters, setFieldFilters] = useState<Record<string, string>>({});

  const [preferences, setPreferences] = useState<Record<string, string[]>>({});
  const [pickerOpen, setPickerOpen] = useState(false);
  const [savingColumns, setSavingColumns] = useState(false);

  // Memoised on the map and the key, not derived inline: a fresh `{}` each render would make
  // every `useMemo` below recompute, and the load effect depends on one of them, so the page
  // would refetch in a loop.
  const activeFields = useMemo(() => fieldsByType[typeFilter] ?? {}, [fieldsByType, typeFilter]);

  /// What the table renders. A stored preference wins; otherwise the schema decides.
  const columns = useMemo(() => {
    if (!typeFilter) return ["__label", "__type", "__created"];
    return preferences[typeFilter] ?? defaultColumns(activeFields);
  }, [typeFilter, preferences, activeFields]);

  /// Only fields whose values are a closed set, since containment matches exactly.
  /// Offering a free-text box over `@>` would look like search and behave like an exact match.
  const filterableFields = useMemo(
    () =>
      Object.entries(activeFields).filter(
        ([, def]) => (def.enum && def.enum.length > 0) || def.type === "boolean",
      ),
    [activeFields],
  );

  const activeFilter = useMemo(() => {
    const out: Record<string, unknown> = {};
    for (const [name, raw] of Object.entries(fieldFilters)) {
      if (!raw) continue;
      const def = activeFields[name];
      out[name] = def?.type === "boolean" ? raw === "true" : raw;
    }
    return out;
  }, [fieldFilters, activeFields]);

  const fetchPage = useCallback(
    async (offset: number, entityType: string, filter: Record<string, unknown>) =>
      listEntities({
        entity_type: entityType || undefined,
        offset,
        limit: PAGE_SIZE,
        filter,
      }),
    [],
  );

  const loadInitial = useCallback(
    async (entityType: string, filter: Record<string, unknown>) => {
      setLoading(true);
      setError(null);
      setSearchActive(false);
      setSearchQuery("");
      try {
        const results = await fetchPage(0, entityType, filter);
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
    loadInitial(typeFilter, activeFilter);
  }, [typeFilter, activeFilter, loadInitial]);

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
        const byType: Record<string, Record<string, FieldDef>> = {};
        for (const detail of details) {
          if (!detail) continue;
          for (const [name, def] of Object.entries(detail.definition.entity_types ?? {})) {
            byType[name] = def.fields ?? {};
          }
        }
        setFieldsByType(byType);
        setEntityTypes(Object.keys(byType).toSorted());
      } catch {
        // Non-fatal: the type dropdown stays empty and the table keeps its built-in columns.
      }
    }
    async function loadPreferences() {
      try {
        const stored = await listColumnPreferences();
        if (cancelled) return;
        setPreferences(Object.fromEntries(stored.map((p) => [p.entity_type, p.columns])));
      } catch {
        // Non-fatal: every type falls back to its schema-derived default.
      }
    }
    loadSchemas();
    loadPreferences();
    return () => {
      cancelled = true;
    };
  }, []);

  async function handleLoadMore() {
    setLoadingMore(true);
    setError(null);
    try {
      const results = await fetchPage(entities.length, typeFilter, activeFilter);
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
      loadInitial(typeFilter, activeFilter);
      return;
    }
    setSearching(true);
    setError(null);
    try {
      const hits = await searchEntities(query, { entity_type: typeFilter || undefined });
      setEntities(hits.map((hit) => hit.entity));
      setHasMore(false);
      setSearchActive(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Search failed");
    } finally {
      setSearching(false);
    }
  }

  async function handleSaveColumns(next: string[]) {
    setSavingColumns(true);
    setError(null);
    try {
      const saved = await setColumnPreference(typeFilter, next);
      setPreferences((prev) => ({ ...prev, [saved.entity_type]: saved.columns }));
      setPickerOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to save columns");
    } finally {
      setSavingColumns(false);
    }
  }

  async function handleResetColumns() {
    setSavingColumns(true);
    setError(null);
    try {
      await resetColumnPreference(typeFilter);
      setPreferences((prev) => {
        const next = { ...prev };
        delete next[typeFilter];
        return next;
      });
      setPickerOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to reset columns");
    } finally {
      setSavingColumns(false);
    }
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
                  onClick={() => {
                    setSearchQuery("");
                    loadInitial(typeFilter, activeFilter);
                  }}
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
              onChange={(e) => {
                setTypeFilter(e.target.value);
                // Filters name fields of the old type, which the new one may not define.
                setFieldFilters({});
              }}
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
            {/* Columns are chosen per entity type, so the button needs one selected. */}
            {typeFilter && <ColumnPickerButton onClick={() => setPickerOpen(true)} />}
          </div>
        </CardHeader>
        <CardContent>
          {filterableFields.length > 0 && (
            <div className="mb-4 flex flex-wrap items-end gap-3">
              {filterableFields.map(([name, def]) => (
                <label key={name} className="flex flex-col gap-1">
                  <span className="text-xs font-medium text-muted-foreground">{name}</span>
                  <select
                    value={fieldFilters[name] ?? ""}
                    onChange={(e) =>
                      setFieldFilters((prev) => ({ ...prev, [name]: e.target.value }))
                    }
                    className="h-9 rounded-md border border-input px-2 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
                    aria-label={`Filter by ${name}`}
                  >
                    <option value="">Any</option>
                    {def.type === "boolean" ? (
                      <>
                        <option value="true">Yes</option>
                        <option value="false">No</option>
                      </>
                    ) : (
                      def.enum?.map((option) => (
                        <option key={option} value={option}>
                          {option}
                        </option>
                      ))
                    )}
                  </select>
                </label>
              ))}
              {Object.values(fieldFilters).some(Boolean) && (
                <Button variant="ghost" size="sm" onClick={() => setFieldFilters({})}>
                  Clear filters
                </Button>
              )}
            </div>
          )}

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
                    {columns.map((column) => (
                      <TableHead key={column}>{columnHeader(column)}</TableHead>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {entities.map((entity) => (
                    <TableRow
                      key={entity.id}
                      className="cursor-pointer"
                      onClick={() => navigate(entity.id)}
                    >
                      {columns.map((column) => (
                        <TableCell key={column} className="max-w-xs truncate">
                          <EntityCell
                            entity={entity}
                            column={column}
                            def={fieldsByType[entity.entity_type]?.[column]}
                          />
                        </TableCell>
                      ))}
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

      {typeFilter && (
        <ColumnPicker
          open={pickerOpen}
          onClose={() => setPickerOpen(false)}
          entityType={typeFilter}
          fields={activeFields}
          selected={columns}
          maxColumns={MAX_COLUMNS}
          saving={savingColumns}
          onSave={handleSaveColumns}
          onReset={handleResetColumns}
        />
      )}
    </div>
  );
}
