import { ReactFlow, ReactFlowProvider, Background, Controls, MiniMap } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { SchemaDefinition } from "@/types/api";
import { schemaNodeTypes, useSchemaGraph } from "@/components/graph/SchemaGraph";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import { useIsDarkMode } from "@/hooks/useIsDarkMode";

interface SchemaStructureCardProps {
  definition: SchemaDefinition | null;
  /// Overrides the default description. The card means the same thing everywhere; only the
  /// wording around "this version" vs "this template" changes.
  description?: string;
}

/// The entity-type/relation graph for one schema definition.
///
/// Every page that shows a schema wants this, and three of them had grown their own copy of the
/// same ReactFlow block -- with two different titles ("Schema Structure" and "Schema Graph") for
/// the identical thing. One component keeps them from drifting further apart.
export function SchemaStructureCard({ definition, description }: SchemaStructureCardProps) {
  const isDark = useIsDarkMode();
  const { nodes, edges } = useSchemaGraph(definition, isDark);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Schema Structure</CardTitle>
        <CardDescription>
          {description ?? "Entity types and their relations for this version."}
        </CardDescription>
      </CardHeader>
      <CardContent className="p-0">
        {nodes.length === 0 ? (
          <div className="flex h-40 items-center justify-center text-sm text-muted-foreground">
            No entity types to display.
          </div>
        ) : (
          <div style={{ height: "24rem" }}>
            <ReactFlowProvider>
              <ReactFlow
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
            </ReactFlowProvider>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
