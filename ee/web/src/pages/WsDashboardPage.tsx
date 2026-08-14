import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { getWorkspace, listEntities } from "@/lib/api";
import type { WorkspaceDetail, Entity } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/Table";
import { Badge } from "@/components/ui/Badge";
import { entityLabel, formatDate } from "@/lib/format";
import { Panel, Stat, UsageBar } from "@/components/ui/Panel";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { useWorkspace } from "@/hooks/useWorkspace";

export function WsDashboardPage() {
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const { workspaceName } = useWorkspace();
  const workspaceId = wsId ?? null;
  const [detail, setDetail] = useState<WorkspaceDetail | null>(null);
  const [recent, setRecent] = useState<Entity[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!workspaceId) return;
    setLoading(true);
    setError(null);
    getWorkspace(workspaceId)
      .then(setDetail)
      .catch((err) => setError(err instanceof Error ? err.message : "Failed to load"))
      .finally(() => setLoading(false));
  }, [workspaceId]);

  // The counters say how much is here; this says what. Without it the workspace's landing page
  // cannot show a single piece of its own content.
  useEffect(() => {
    if (!workspaceId) return;
    let cancelled = false;
    listEntities({ limit: 5 })
      .then((data) => {
        if (!cancelled) setRecent(data);
      })
      .catch(() => {
        // A failure here leaves the panel empty rather than replacing the whole page with an
        // error -- the counters above it are still worth showing.
      });
    return () => {
      cancelled = true;
    };
  }, [workspaceId]);

  if (loading) return <PageSkeleton />;

  if (error || !detail) {
    return (
      <div className="p-6">
        <Card>
          <CardContent className="pt-6">
            <p className="text-sm text-destructive">{error ?? "Workspace not found"}</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Relations per entity says more about the shape of the graph than either count alone: near
  // zero means entities were imported without their links.
  const density =
    detail.entity_count > 0 ? (detail.relation_count / detail.entity_count).toFixed(2) : "0";

  return (
    <div className="space-y-4 p-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{workspaceName ?? "Workspace"}</h1>
        <p className="text-sm text-muted-foreground">Workspace overview and quick access.</p>
      </div>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Panel title="Entities">
          <Stat value={detail.entity_count} limit={detail.max_entities} />
          <UsageBar value={detail.entity_count} limit={detail.max_entities} />
        </Panel>
        <Panel title="Relations">
          <Stat value={detail.relation_count} caption={`${density} per entity`} />
        </Panel>
        <Panel title="Schemas">
          <Stat value={detail.schema_count} caption="versions in this workspace" />
        </Panel>
        <Panel title="Entity quota">
          <Stat
            value={detail.max_entities ?? "Unlimited"}
            caption={
              detail.max_entities
                ? `${Math.max(detail.max_entities - detail.entity_count, 0).toLocaleString()} remaining`
                : "no cap configured"
            }
          />
        </Panel>
      </div>

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between gap-2">
            <div>
              <CardTitle className="text-lg">Recent entities</CardTitle>
              <CardDescription>The five most recently created in this workspace.</CardDescription>
            </div>
            <Link
              to={`/ws/${wsId}/entities`}
              className="shrink-0 text-sm font-medium text-link hover:underline"
            >
              View all
            </Link>
          </div>
        </CardHeader>
        <CardContent>
          {recent.length === 0 ? (
            <p className="py-6 text-center text-sm text-muted-foreground">
              No entities yet.{" "}
              <Link to={`/ws/${wsId}/entities/new`} className="text-link hover:underline">
                Create the first one
              </Link>
              .
            </p>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead>Created</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {recent.map((entity) => (
                    <TableRow
                      key={entity.id}
                      className="cursor-pointer"
                      onClick={() => navigate(`/ws/${wsId}/entities/${entity.id}`)}
                    >
                      <TableCell className="font-medium">{entityLabel(entity)}</TableCell>
                      <TableCell>
                        <Badge variant="secondary">{entity.entity_type}</Badge>
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
        </CardContent>
      </Card>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
        <Link to={`/ws/${wsId}/entities`}>
          <Card className="cursor-pointer transition-colors hover:bg-accent/5">
            <CardContent className="pt-6">
              <p className="font-medium">Entities</p>
              <p className="text-sm text-muted-foreground">Browse and create entities</p>
            </CardContent>
          </Card>
        </Link>
        <Link to={`/ws/${wsId}/graph`}>
          <Card className="cursor-pointer transition-colors hover:bg-accent/5">
            <CardContent className="pt-6">
              <p className="font-medium">Graph</p>
              <p className="text-sm text-muted-foreground">
                Visualize schema structure and entity relations
              </p>
            </CardContent>
          </Card>
        </Link>
        <Link to={`/ws/${wsId}/schema`}>
          <Card className="cursor-pointer transition-colors hover:bg-accent/5">
            <CardContent className="pt-6">
              <p className="font-medium">Schema</p>
              <p className="text-sm text-muted-foreground">
                View this workspace's schema definition
              </p>
            </CardContent>
          </Card>
        </Link>
        <Link to={`/ws/${wsId}/search`}>
          <Card className="cursor-pointer transition-colors hover:bg-accent/5">
            <CardContent className="pt-6">
              <p className="font-medium">Search</p>
              <p className="text-sm text-muted-foreground">Search entities by text similarity</p>
            </CardContent>
          </Card>
        </Link>
        <Link to={`/ws/${wsId}/schema/io`}>
          <Card className="cursor-pointer transition-colors hover:bg-accent/5">
            <CardContent className="pt-6">
              <p className="font-medium">Import / Export</p>
              <p className="text-sm text-muted-foreground">Import and export workspace data</p>
            </CardContent>
          </Card>
        </Link>
      </div>
    </div>
  );
}
