import { useEffect, useMemo, useState } from "react";
import { Handle, Position } from "@xyflow/react";
import ELK from "elkjs/lib/elk.bundled.js";
import { KeyRound, Boxes } from "lucide-react";
import type { SchemaDefinition } from "@/types/api";
import { cn } from "@/lib/cn";

export interface SchemaNodeField {
  name: string;
  type: string;
  required: boolean;
  embed: boolean;
}

export interface SchemaNodeData {
  label: string;
  fields: SchemaNodeField[];
  description: string | null;
  [key: string]: unknown;
}

/// Field type colours, per theme.
///
/// A single shade cannot serve both card backgrounds -- at `-500` these read between 2.1 and
/// 4.2 on white, and the shades dark enough to fix that (`-700`) drop to 2.5-3.5 on `#18181b`.
/// The `dark:` variant picks the light end for dark cards and vice versa.
function fieldTypeColor(type: string): string {
  switch (type) {
    case "string":
      return "text-emerald-700 dark:text-emerald-400";
    case "integer":
    case "number":
      return "text-blue-700 dark:text-blue-400";
    case "boolean":
      return "text-amber-700 dark:text-amber-400";
    case "array":
      return "text-violet-700 dark:text-violet-400";
    case "object":
      return "text-rose-700 dark:text-rose-400";
    default:
      return "text-muted-foreground";
  }
}

export function SchemaNode({ data }: { data: SchemaNodeData }) {
  return (
    <div className="flex min-w-[240px] max-w-[300px] flex-col rounded-xl border border-border bg-card shadow-md">
      <Handle type="target" position={Position.Left} className="!bg-primary" />
      <Handle type="source" position={Position.Right} className="!bg-primary" />

      <div className="flex items-center justify-between gap-2 rounded-t-xl bg-primary px-3 py-2">
        <div className="truncate text-sm font-semibold text-primary-foreground">{data.label}</div>
        {/* /5, not /20: the pill's white overlay lightens the indigo underneath it, and at
            20% that took white-on-primary down to 4.2 (3.7 in dark). Still reads as a pill. */}
        <span className="shrink-0 rounded-full bg-primary-foreground/5 px-2 py-0.5 text-[10px] font-medium text-primary-foreground">
          {data.fields.length} {data.fields.length === 1 ? "field" : "fields"}
        </span>
      </div>

      <div className="max-h-56 overflow-y-auto px-3 py-2">
        {data.fields.length === 0 ? (
          <div className="text-xs text-muted-foreground">No fields</div>
        ) : (
          <ul className="space-y-1">
            {data.fields.map((f) => (
              <li key={f.name} className="flex items-center gap-1.5 text-xs">
                <span className="flex w-3.5 shrink-0 items-center justify-center">
                  {f.required ? (
                    <KeyRound className="h-3 w-3 text-muted-foreground" />
                  ) : f.embed ? (
                    <Boxes className="h-3 w-3 text-muted-foreground" />
                  ) : null}
                </span>
                <span className="truncate font-mono">{f.name}</span>
                <span className={cn("ml-auto shrink-0 font-mono", fieldTypeColor(f.type))}>
                  {f.type}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>

      {data.description && (
        <div className="truncate rounded-b-xl border-t border-border px-3 py-1.5 text-xs italic text-muted-foreground">
          {data.description}
        </div>
      )}
    </div>
  );
}

export const schemaNodeTypes = { entityType: SchemaNode };

const elk = new ELK();

function estimateNodeHeight(fieldCount: number, hasDescription: boolean): number {
  const header = 36;
  const fieldHeight = Math.min(fieldCount, 10) * 20;
  const desc = hasDescription ? 28 : 0;
  return header + Math.max(fieldHeight, 24) + desc;
}

/// Edge colours, one palette per theme.
///
/// These are drawn on the canvas itself rather than on a card, so they are read against
/// `--color-background` at both ends of the range. A single palette cannot serve both: the
/// bright end (emerald at 2.4 on a light canvas) and the dark end (indigo at 4.0 on a dark
/// one) both fail, and no hex in these six hues clears 4.5 against `#09090b` *and* `#fafafa`
/// -- the best any of them manages is about 4.2. Same six hues, lightened for dark and
/// deepened for light, so a relation keeps its identity when the theme changes.
const RELATION_PALETTE_DARK = ["#818cf8", "#34d399", "#fbbf24", "#f472b6", "#22d3ee", "#a78bfa"];
const RELATION_PALETTE_LIGHT = ["#4f46e5", "#047857", "#b45309", "#be185d", "#0e7490", "#6d28d9"];

export function relationPalette(isDark: boolean): string[] {
  return isDark ? RELATION_PALETTE_DARK : RELATION_PALETTE_LIGHT;
}

interface GraphResult {
  nodes: Array<{
    id: string;
    type: "entityType";
    position: { x: number; y: number };
    data: SchemaNodeData;
  }>;
  edges: Array<{
    id: string;
    source: string;
    target: string;
    type: "smoothstep";
    animated: boolean;
    label: string;
    style: { stroke: string };
    labelStyle: { fill: string; fontSize: number };
    labelBgStyle: { fill: string; fillOpacity: number };
    markerEnd: { type: "arrowclosed"; color: string };
  }>;
}

export function useSchemaGraph(definition: SchemaDefinition | null, isDark: boolean): GraphResult {
  const rawGraph = useMemo(() => {
    if (!definition) return null;
    const palette = relationPalette(isDark);

    const entityTypes = Object.entries(definition.entity_types ?? {});
    const relationTypes = Object.entries(definition.relation_types ?? {});

    const nodes = entityTypes.map(([name, def]) => {
      const fields = Object.entries(def.fields ?? {}).map(([fname, fdef]) => ({
        name: fname,
        type: fdef.type,
        required: fdef.required,
        embed: Boolean(fdef["x-embed"]),
      }));
      return {
        id: name,
        type: "entityType" as const,
        position: { x: 0, y: 0 },
        data: { label: name, fields, description: def.description },
        width: 280,
        height: estimateNodeHeight(fields.length, def.description !== null),
      };
    });

    const edges = relationTypes.map(([relName, rel], i) => ({
      id: `rel-${relName}`,
      source: rel.source,
      target: rel.target,
      type: "smoothstep" as const,
      animated: true,
      label: relName,
      style: { stroke: palette[i % palette.length] },
      labelStyle: { fill: palette[i % palette.length], fontSize: 11 },
      labelBgStyle: { fill: "var(--color-background)", fillOpacity: 0.8 },
      markerEnd: {
        type: "arrowclosed" as const,
        color: palette[i % palette.length],
      },
    }));

    return { nodes, edges };
  }, [definition, isDark]);

  const [layoutResult, setLayoutResult] = useState<GraphResult>({ nodes: [], edges: [] });

  useEffect(() => {
    if (!rawGraph || rawGraph.nodes.length === 0) {
      setLayoutResult({ nodes: [], edges: [] });
      return;
    }

    let cancelled = false;

    elk
      .layout({
        id: "root",
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "RIGHT",
          "elk.spacing.nodeNode": "60",
          "elk.layered.spacing.nodeNodeBetweenLayers": "120",
          "elk.edgeRouting": "SPLINES",
          "elk.layered.crossingMinimization.strategy": "LAYER_SWEEP",
        },
        children: rawGraph.nodes.map((n) => ({
          id: n.id,
          width: n.width,
          height: n.height,
        })),
        edges: rawGraph.edges.map((e) => ({
          id: e.id,
          sources: [e.source],
          targets: [e.target],
        })),
      })
      .then((layouted) => {
        if (cancelled) return;
        const posMap = new Map<string, { x: number; y: number }>();
        for (const child of layouted.children ?? []) {
          posMap.set(child.id, { x: child.x ?? 0, y: child.y ?? 0 });
        }
        const nodes = rawGraph.nodes.map((n) => ({
          ...n,
          position: posMap.get(n.id) ?? n.position,
        }));
        setLayoutResult({ nodes, edges: rawGraph.edges });
      })
      // ELK runs the layout in a worker, so a failure here rejects asynchronously and no
      // enclosing try/catch can see it. The nodes carry `{x: 0, y: 0}` until ELK assigns
      // positions, so falling back to them draws the whole graph stacked on one point, which is
      // no more readable than the empty canvas an unhandled rejection leaves. A grid puts every
      // node somewhere distinct: the relations are lost but the entity types are all readable.
      .catch(() => {
        if (cancelled) return;
        const columns = Math.ceil(Math.sqrt(rawGraph.nodes.length));
        const nodes = rawGraph.nodes.map((n, i) => ({
          ...n,
          position: { x: (i % columns) * 340, y: Math.floor(i / columns) * 220 },
        }));
        setLayoutResult({ nodes, edges: rawGraph.edges });
      });

    return () => {
      cancelled = true;
    };
  }, [rawGraph]);

  return layoutResult;
}
