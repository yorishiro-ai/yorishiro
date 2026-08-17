import { useEffect, useState, useCallback, useRef } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  Controls,
  MiniMap,
  Handle,
  Position,
  MarkerType,
  useReactFlow,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { RefreshCw, Search, Maximize2 } from "lucide-react";
import { listSchemas, getActiveSchema, listEntities, getEntityContext } from "@/lib/api";
import type { Schema, SchemaDetail, Entity, Relation } from "@/types/api";
import { schemaNodeTypes, useSchemaGraph, relationPalette } from "@/components/graph/SchemaGraph";
import { Card, CardContent } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Badge } from "@/components/ui/Badge";
import { Skeleton } from "@/components/ui/Skeleton";
import { cn } from "@/lib/cn";
import { entityLabel } from "@/lib/format";
import { useIsDarkMode } from "@/hooks/useIsDarkMode";

// Shared style block so ReactFlow chrome (controls / minimap / background)
// follows the app's theme tokens instead of the library defaults.
const DARK_FLOW_STYLES = `
.dark-flow .react-flow__background {
  stroke: var(--color-border);
}
.dark-flow .react-flow__controls {
  background: var(--color-card);
  border: 1px solid var(--color-border);
  border-radius: 0.5rem;
  overflow: hidden;
  box-shadow: none;
}
.dark-flow .react-flow__controls button {
  background: var(--color-card);
  color: var(--color-foreground);
  border-color: var(--color-border);
}
.dark-flow .react-flow__controls button:hover {
  background: var(--color-accent);
}
.dark-flow .react-flow__controls button path {
  fill: currentColor;
}
.dark-flow .react-flow__minimap {
  background: var(--color-card);
}
.dark-flow .react-flow__attribution {
  background: transparent;
  color: var(--color-muted-foreground);
}
`;

// ── Tab toggle ───────────────────────────────────────────────

type TabId = "schema" | "entity";

// ── Schema structure tab ─────────────────────────────────────

// ── Entity graph tab ─────────────────────────────────────────

interface EntityNodeData {
  label: string;
  entityType: string;
  isRoot: boolean;
  entityId: string;
  preview: Array<[string, string]>;
  onNavigate: (id: string) => void;
  [key: string]: unknown;
}

function EntityNode({ data }: { data: EntityNodeData }) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => data.onNavigate(data.entityId)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") data.onNavigate(data.entityId);
      }}
      className={cn(
        "cursor-pointer rounded-xl border bg-card shadow-md transition-transform hover:-translate-y-0.5",
        data.isRoot
          ? "min-w-[180px] animate-pulse border-primary ring-2 ring-primary px-4 py-3"
          : "min-w-[140px] border-border px-3 py-2",
      )}
    >
      <Handle type="target" position={Position.Top} className="!bg-primary" />
      <Handle type="source" position={Position.Bottom} className="!bg-primary" />

      <div className="flex items-center gap-1.5">
        <Badge variant={data.isRoot ? "default" : "secondary"} className="text-[10px]">
          {data.entityType}
        </Badge>
        {data.isRoot && (
          <span className="text-[10px] font-semibold uppercase tracking-wide text-link">root</span>
        )}
      </div>
      <div className="mt-1 truncate text-xs font-medium">{data.label}</div>
      <div className="truncate font-mono text-[10px] text-muted-foreground">
        {data.entityId.slice(0, 8)}
      </div>

      {data.preview.length > 0 && (
        <div className="mt-1.5 space-y-0.5 border-t border-border pt-1.5">
          {data.preview.map(([k, v]) => (
            <div key={k} className="flex gap-1 truncate text-[10px]">
              <span className="shrink-0 text-muted-foreground">{k}:</span>
              <span className="truncate font-medium">{v}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

const entityNodeTypes = { entity: EntityNode };

// ── Layout helpers ───────────────────────────────────────────

function colorForRelationType(
  relationType: string,
  order: Map<string, number>,
  isDark: boolean,
): string {
  if (!order.has(relationType)) order.set(relationType, order.size);
  const palette = relationPalette(isDark);
  return palette[order.get(relationType)! % palette.length];
}

// ── Page ─────────────────────────────────────────────────────

export function GraphPage() {
  const [tab, setTab] = useState<TabId>("schema");
  const isDark = useIsDarkMode();

  return (
    <div className="flex h-full flex-col gap-4">
      <style dangerouslySetInnerHTML={{ __html: DARK_FLOW_STYLES }} />

      <div className="flex items-center justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">Graph</h1>
        <div className="flex rounded-lg border border-border bg-secondary p-0.5">
          <button
            type="button"
            onClick={() => setTab("schema")}
            className={cn(
              "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
              tab === "schema"
                ? "bg-card shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            Schema Structure
          </button>
          <button
            type="button"
            onClick={() => setTab("entity")}
            className={cn(
              "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
              tab === "entity"
                ? "bg-card shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            Entity Graph
          </button>
        </div>
      </div>

      <ReactFlowProvider>
        {tab === "schema" ? <SchemaTab isDark={isDark} /> : <EntityTab isDark={isDark} />}
      </ReactFlowProvider>
    </div>
  );
}

// ── Toolbar "Fit View" button ────────────────────────────────

function FitViewButton() {
  const { fitView } = useReactFlow();
  return (
    <Button variant="secondary" size="sm" onClick={() => fitView({ padding: 0.2, duration: 300 })}>
      <Maximize2 className="h-4 w-4" />
      Fit View
    </Button>
  );
}

// ── Schema tab ───────────────────────────────────────────────

function SchemaTab({ isDark }: { isDark: boolean }) {
  const [schemas, setSchemas] = useState<Schema[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedName, setSelectedName] = useState("");
  const [detail, setDetail] = useState<SchemaDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listSchemas();
      setSchemas(data);
      if (data.length > 0 && !selectedName) setSelectedName(data[0].name);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load");
    } finally {
      setLoading(false);
    }
  }, [selectedName]);

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!selectedName) return;
    let cancelled = false;
    setDetailLoading(true);
    getActiveSchema(selectedName)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch(() => {
        if (!cancelled) setDetail(null);
      })
      .finally(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedName]);

  const { nodes, edges } = useSchemaGraph(detail?.definition ?? null, isDark);

  if (loading) return <Skeleton className="h-96 w-full" />;
  if (error) return <div className="text-sm text-destructive">{error}</div>;

  return (
    <>
      <div className="flex items-center gap-3">
        <select
          value={selectedName}
          onChange={(e) => setSelectedName(e.target.value)}
          className="rounded-md border border-input bg-card px-3 py-1.5 text-sm"
        >
          {schemas.map((s) => (
            <option key={s.id} value={s.name}>
              {s.name} (v{s.version})
            </option>
          ))}
        </select>
        <Button variant="ghost" size="sm" onClick={load} aria-label="Reload schema structure">
          <RefreshCw className="h-4 w-4" />
        </Button>
        <div className="ml-auto">
          <FitViewButton />
        </div>
      </div>

      {/* No `flex-1` here. This card sets its own height, and `flex-1` (basis 0, grow 1) inside
          a parent that has no height of its own beats the inline value -- the canvas computed to
          26px, and React Flow inside it to zero, so the graph was invisible while its nodes
          existed in the DOM. */}
      <Card className="flex flex-col overflow-hidden" style={{ height: "calc(100vh - 260px)" }}>
        <CardContent className="flex-1 p-0">
          {detailLoading ? (
            <div className="flex h-full items-center justify-center">
              <Skeleton className="h-40 w-60" />
            </div>
          ) : nodes.length === 0 ? (
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              No entity types to display.
            </div>
          ) : (
            <ReactFlow
              className={isDark ? "dark-flow" : undefined}
              nodes={nodes}
              edges={edges}
              nodeTypes={schemaNodeTypes}
              fitView
              fitViewOptions={{ padding: 0.2 }}
              minZoom={0.1}
              maxZoom={2}
            >
              <Background gap={16} />
              <Controls position="bottom-right" />
              <MiniMap
                pannable
                zoomable
                nodeColor="var(--color-primary)"
                maskColor="rgba(0,0,0,0.15)"
              />
            </ReactFlow>
          )}
        </CardContent>
      </Card>
    </>
  );
}

// ── Entity graph tab ─────────────────────────────────────────

interface EntityFlowNode {
  id: string;
  type: "entity";
  position: { x: number; y: number };
  data: EntityNodeData;
}

interface EntityFlowEdge {
  id: string;
  source: string;
  target: string;
  label: string;
  type?: string;
  style?: Record<string, unknown>;
  labelStyle?: Record<string, unknown>;
  labelBgPadding?: [number, number];
  labelBgBorderRadius?: number;
  labelBgStyle?: Record<string, unknown>;
  markerEnd: { type: MarkerType; color?: string };
}

function radialPosition(hop: number, index: number, countAtHop: number) {
  if (hop === 0) return { x: 400, y: 300 };
  const radius = hop * 200;
  const angle = (2 * Math.PI * index) / Math.max(countAtHop, 1);
  return {
    x: 400 + radius * Math.cos(angle),
    y: 300 + radius * Math.sin(angle),
  };
}

function entityPreview(entity: Entity | undefined): Array<[string, string]> {
  if (!entity || !entity.data) return [];
  return (
    Object.entries(entity.data)
      .slice(0, 2)
      // An object goes through JSON so it renders as its shape rather than "[object Object]".
      // Everything else goes through `String()`, `undefined` included: `JSON.stringify(undefined)`
      // answers `undefined` rather than a string, which this function's return type forbids.
      .map(([k, v]) => [k, typeof v === "object" && v !== null ? JSON.stringify(v) : String(v)])
  );
}

/// How many entities the picker offers.
/// A graph is read one entity at a time, so this bounds the dropdown rather than the view: a
/// workspace with more of them is normal, and the search page is how you reach the rest.
const ENTITY_PICKER_LIMIT = 100;

function EntityTab({ isDark }: { isDark: boolean }) {
  const navigate = useNavigate();
  const { wsId } = useParams<{ wsId: string }>();
  const [entities, setEntities] = useState<Entity[]>([]);
  const [loading, setLoading] = useState(true);
  // Distinct from `entities.length === 0`: a failed load and an empty workspace both leave the
  // list empty, and rendering "no entities yet" over a failure tells the reader to create
  // something when the real answer is to retry.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState("");
  const [depth, setDepth] = useState(2);
  const [graphLoading, setGraphLoading] = useState(false);
  const [graphNodes, setGraphNodes] = useState<EntityFlowNode[]>([]);
  const [graphEdges, setGraphEdges] = useState<EntityFlowEdge[]>([]);
  const relationColorOrder = useRef(new Map<string, number>());

  const handleNavigate = useCallback(
    (id: string) => {
      navigate(`/ws/${wsId}/entities/${id}`);
    },
    [navigate, wsId],
  );

  const loadEntities = useCallback(async (isCancelled: () => boolean) => {
    setLoading(true);
    setLoadError(null);
    try {
      const data = await listEntities({ limit: ENTITY_PICKER_LIMIT });
      if (isCancelled()) return;
      setEntities(data);
      if (data.length === 0) return;

      // Open on an entity that actually has neighbours. Picking `data[0]` blindly lands on an
      // isolated entity whenever the newest one has no relations yet, and the page then shows
      // a single node with nothing joining it -- indistinguishable from a broken graph.
      // Bounded to the first handful so this stays one quick pass, not a scan of every entity.
      const probes = data.slice(0, 8);
      const contexts = await Promise.all(
        probes.map((e) =>
          getEntityContext(e.id, 1)
            .then((ctx) => ({ id: e.id, count: ctx.relations.length }))
            .catch(() => ({ id: e.id, count: 0 })),
        ),
      );
      if (isCancelled()) return;
      const connected = contexts.find((c) => c.count > 0);
      setSelectedId((prev) => prev || connected?.id || data[0].id);
    } catch (e) {
      if (isCancelled()) return;
      setEntities([]);
      setLoadError(e instanceof Error ? e.message : "Failed to load entities");
    } finally {
      if (!isCancelled()) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    loadEntities(() => cancelled);
    return () => {
      cancelled = true;
    };
  }, [loadEntities]);

  const loadGraph = useCallback(async () => {
    if (!selectedId) return;
    setGraphLoading(true);
    try {
      const ctx = await getEntityContext(selectedId, depth);
      const allIds = new Set<string>();
      allIds.add(ctx.entity.id);
      const edgeList: Array<{ id: string; source: string; target: string; label: string }> = [];

      ctx.relations.forEach((r: Relation, i: number) => {
        allIds.add(r.neighbor.id);
        const src = r.direction === "out" ? ctx.entity.id : r.neighbor.id;
        const tgt = r.direction === "out" ? r.neighbor.id : ctx.entity.id;
        edgeList.push({
          id: `rel-${i}`,
          source: src,
          target: tgt,
          label: r.relation_type,
        });
      });

      // Group ids by hop distance for the radial layout.
      const hopOf = new Map<string, number>();
      hopOf.set(ctx.entity.id, 0);
      for (const r of ctx.relations) {
        const existing = hopOf.get(r.neighbor.id);
        if (existing === undefined || r.hop_distance < existing) {
          hopOf.set(r.neighbor.id, r.hop_distance);
        }
      }

      const byHop = new Map<number, string[]>();
      for (const id of allIds) {
        const hop = hopOf.get(id) ?? 1;
        if (!byHop.has(hop)) byHop.set(hop, []);
        byHop.get(hop)!.push(id);
      }

      relationColorOrder.current = new Map();

      const nodes: EntityFlowNode[] = [];
      for (const [hop, ids] of byHop) {
        ids.forEach((id, i) => {
          const rel = ctx.relations.find((r: Relation) => r.neighbor.id === id);
          const isRoot = id === ctx.entity.id;
          nodes.push({
            id,
            type: "entity",
            position: radialPosition(hop, i, ids.length),
            data: {
              // Neighbours carry their own data, so they can be named the same way the root
              // is rather than being shown as an id prefix that repeats across every node.
              label: isRoot
                ? entityLabel(ctx.entity)
                : rel?.neighbor
                  ? entityLabel(rel.neighbor)
                  : id.slice(0, 8),
              entityType: isRoot
                ? ctx.entity.entity_type
                : (rel?.neighbor.entity_type ?? "unknown"),
              isRoot,
              entityId: id,
              preview: isRoot ? entityPreview(ctx.entity) : entityPreview(rel?.neighbor),
              onNavigate: handleNavigate,
            },
          });
        });
      }

      const edges: EntityFlowEdge[] = edgeList.map((e) => {
        const color = colorForRelationType(e.label, relationColorOrder.current, isDark);
        return {
          ...e,
          markerEnd: { type: MarkerType.ArrowClosed, color },
          style: { stroke: color, strokeWidth: 1.5 },
          labelStyle: { fill: color, fontSize: 11, fontWeight: 600 },
          labelBgPadding: [6, 3] as [number, number],
          labelBgBorderRadius: 6,
          labelBgStyle: { fill: "var(--color-card)", fillOpacity: 0.9 },
        };
      });

      setGraphNodes(nodes);
      setGraphEdges(edges);
    } finally {
      setGraphLoading(false);
    }
    // isDark: edge colours come from the theme-specific palette, so switching theme has to
    // rebuild them -- without it the graph keeps the previous theme's colours until reload.
  }, [selectedId, depth, handleNavigate, isDark]);

  useEffect(() => {
    if (selectedId) loadGraph();
  }, [selectedId, depth, loadGraph]);

  if (loading) return <Skeleton className="h-96 w-full" />;

  return (
    <>
      <div className="flex items-center gap-3">
        <div className="flex items-center gap-2">
          <Search className="h-4 w-4 text-muted-foreground" />
          <select
            value={selectedId}
            onChange={(e) => setSelectedId(e.target.value)}
            disabled={entities.length === 0}
            aria-label="Entity to visualize"
            className="rounded-md border border-input bg-card px-3 py-1.5 text-sm disabled:opacity-60"
          >
            {/* An empty select renders as a blank box, which reads as a broken control rather
                than an empty workspace. */}
            {entities.length === 0 ? (
              <option value="">No entities yet</option>
            ) : (
              entities.map((ent) => (
                <option key={ent.id} value={ent.id}>
                  {ent.entity_type} — {entityLabel(ent)}
                </option>
              ))
            )}
          </select>
          {/* The picker holds the first ENTITY_PICKER_LIMIT entities and nothing says so, which
              reads as "this entity does not exist" rather than "this list is cut off". A full
              count is not fetched to say it: hitting the limit is the same evidence, and the
              search page is where finding one by name belongs. */}
          {entities.length >= ENTITY_PICKER_LIMIT && (
            <span className="text-xs text-muted-foreground">
              first {ENTITY_PICKER_LIMIT}; search for others
            </span>
          )}
        </div>
        <div className="flex items-center gap-2 text-sm">
          <span className="text-muted-foreground">Depth:</span>
          {[1, 2, 3].map((d) => (
            <button
              key={d}
              type="button"
              onClick={() => setDepth(d)}
              className={cn(
                "rounded-md px-2 py-1 text-sm font-medium transition-colors",
                depth === d
                  ? "bg-primary text-primary-foreground"
                  : "bg-secondary text-secondary-foreground hover:bg-accent",
              )}
            >
              {d}
            </button>
          ))}
        </div>
        <Button variant="ghost" size="sm" onClick={loadGraph} aria-label="Reload entity graph">
          <RefreshCw className="h-4 w-4" />
        </Button>
        <div className="ml-auto">
          <FitViewButton />
        </div>
      </div>

      {/* No `flex-1` here. This card sets its own height, and `flex-1` (basis 0, grow 1) inside
          a parent that has no height of its own beats the inline value -- the canvas computed to
          26px, and React Flow inside it to zero, so the graph was invisible while its nodes
          existed in the DOM. */}
      <Card className="flex flex-col overflow-hidden" style={{ height: "calc(100vh - 260px)" }}>
        <CardContent className="flex-1 p-0">
          {graphLoading ? (
            <div className="flex h-full items-center justify-center">
              <Skeleton className="h-40 w-60" />
            </div>
          ) : loadError ? (
            // Checked before the empty case, since a failed load also leaves the list empty and
            // "create one" would be the wrong instruction for a request that never answered.
            <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
              <p className="text-sm font-medium text-destructive">{loadError}</p>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => {
                  loadEntities(() => false);
                }}
              >
                Retry
              </Button>
            </div>
          ) : entities.length === 0 ? (
            // "Select an entity" is wrong when there is nothing to select: the workspace is
            // empty, and the next step is creating something, not choosing.
            <div className="flex h-full flex-col items-center justify-center gap-1 px-6 text-center">
              <p className="text-sm font-medium">This workspace has no entities yet.</p>
              <p className="text-sm text-muted-foreground">
                <Link to={`/ws/${wsId}/entities/new`} className="text-link hover:underline">
                  Create one
                </Link>{" "}
                and it will appear here.
              </p>
            </div>
          ) : graphNodes.length === 0 ? (
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              Select an entity to visualize its connections.
            </div>
          ) : graphEdges.length === 0 ? (
            // A lone root node renders as a single card with nothing joining it, which reads as
            // a graph that failed to load rather than an entity that has no relations yet.
            <div className="flex h-full flex-col items-center justify-center gap-1 px-6 text-center">
              <p className="text-sm font-medium">This entity has no relations.</p>
              <p className="text-sm text-muted-foreground">
                Nothing links to or from it{depth > 1 ? ` within ${depth} hops` : ""}. Pick another
                entity, or create a relation to see it here.
              </p>
            </div>
          ) : (
            <ReactFlow
              className={isDark ? "dark-flow" : undefined}
              nodes={graphNodes}
              edges={graphEdges}
              nodeTypes={entityNodeTypes}
              fitView
              fitViewOptions={{ padding: 0.3 }}
              minZoom={0.1}
              maxZoom={2}
            >
              <Background gap={16} />
              <Controls position="bottom-right" />
              <MiniMap
                pannable
                zoomable
                nodeColor="var(--color-primary)"
                maskColor="rgba(0,0,0,0.15)"
              />
            </ReactFlow>
          )}
        </CardContent>
      </Card>
    </>
  );
}
