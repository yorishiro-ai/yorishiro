import { useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import {
  getEntity,
  getEntityContext,
  updateEntity,
  deleteEntity,
  listSchemas,
  getActiveSchema,
} from "@/lib/api";
import type { Entity, EntityContext, EntityTypeDef } from "@/types/api";
import { EntityForm } from "@/components/entities/EntityForm";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/Table";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { Badge } from "@/components/ui/Badge";
import { Skeleton, PageSkeleton } from "@/components/ui/Skeleton";
import { formatDateTime, truncateId } from "@/lib/format";

export function EntityDetailPage() {
  const { id, wsId } = useParams<{ id: string; wsId: string }>();
  const navigate = useNavigate();

  const [entity, setEntity] = useState<Entity | null>(null);
  const [context, setContext] = useState<EntityContext | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [editing, setEditing] = useState(false);
  const [editJsonMode, setEditJsonMode] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [entityTypeDef, setEntityTypeDef] = useState<EntityTypeDef | null>(null);

  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const load = useCallback((entityId: string) => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    Promise.all([getEntity(entityId), getEntityContext(entityId)])
      .then(([entityData, contextData]) => {
        if (cancelled) return;
        setEntity(entityData);
        setContext(contextData);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "Failed to load entity");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!id) return;
    const cancel = load(id);
    return cancel;
  }, [id, load]);

  async function handleStartEdit(jsonMode: boolean) {
    if (!entity) return;
    setSaveError(null);
    setEditJsonMode(jsonMode);
    if (!jsonMode) {
      try {
        const schemas = await listSchemas();
        const active = schemas.filter((s) => s.status === "active");
        for (const s of active) {
          const detail = await getActiveSchema(s.name);
          const typeDef = detail.definition.entity_types[entity.entity_type];
          if (typeDef) {
            setEntityTypeDef(typeDef);
            break;
          }
        }
      } catch {
        setEditJsonMode(true);
      }
    }
    setEditing(true);
  }

  function handleCancelEdit() {
    setEditing(false);
    setEditJsonMode(false);
    setSaveError(null);
    setEntityTypeDef(null);
  }

  async function handleSave(data: Record<string, unknown>) {
    if (!id) return;
    setSaveError(null);
    setSaving(true);
    try {
      const updated = await updateEntity(id, data);
      setEntity(updated);
      setEditing(false);
      setEntityTypeDef(null);
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : "Failed to save entity");
    } finally {
      setSaving(false);
    }
  }

  async function handleDelete() {
    if (!id) return;
    setDeleteError(null);
    setDeleting(true);
    try {
      await deleteEntity(id);
      navigate(`/ws/${wsId}/entities`);
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : "Failed to delete entity");
      setDeleting(false);
    }
  }

  if (loading) {
    return <PageSkeleton />;
  }

  if (error || !entity) {
    return (
      <div className="space-y-4 p-6">
        <BackLink wsId={wsId} />
        <Card>
          <CardContent className="p-6">
            <p className="text-sm text-destructive">{error ?? "Entity not found."}</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  const relations = context?.relations ?? [];

  return (
    <div className="space-y-6 p-6">
      <BackLink wsId={wsId} />

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between gap-4">
            <div className="flex items-center gap-3">
              <CardTitle className="font-mono text-lg">{truncateId(entity.id)}</CardTitle>
              <Badge variant="secondary">{entity.entity_type}</Badge>
            </div>
            <Button
              variant="destructive"
              size="sm"
              onClick={() => {
                setDeleteError(null);
                setDeleteOpen(true);
              }}
            >
              Delete
            </Button>
          </div>
          <CardDescription>Entity metadata and relations.</CardDescription>
        </CardHeader>
        <CardContent>
          <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <div>
              <dt className="text-xs font-medium text-muted-foreground">ID</dt>
              <dd className="break-all font-mono text-sm" title={entity.id}>
                {truncateId(entity.id)}
              </dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Entity Type</dt>
              <dd className="text-sm">
                <Badge variant="secondary">{entity.entity_type}</Badge>
              </dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Schema Version</dt>
              <dd className="text-sm">{entity.schema_version}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Workspace</dt>
              <dd
                className="break-all font-mono text-xs text-muted-foreground"
                title={entity.workspace_id}
              >
                {truncateId(entity.workspace_id)}
              </dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Created At</dt>
              <dd className="text-sm">{formatDateTime(entity.created_at)}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Updated At</dt>
              <dd className="text-sm">{formatDateTime(entity.updated_at)}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>

      <Card>
        <CardHeader className="flex flex-row items-center justify-between">
          <div>
            <CardTitle className="text-lg">Data</CardTitle>
            <CardDescription>Entity payload.</CardDescription>
          </div>
          {!editing && (
            <div className="flex gap-2">
              <Button variant="secondary" size="sm" onClick={() => handleStartEdit(false)}>
                Edit
              </Button>
              <Button variant="ghost" size="sm" onClick={() => handleStartEdit(true)}>
                Edit as JSON
              </Button>
            </div>
          )}
        </CardHeader>
        <CardContent>
          {saveError && (
            <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {saveError}
            </div>
          )}
          {editing ? (
            <EntityForm
              entityTypeDef={entityTypeDef ?? { description: null, fields: {} }}
              initialData={entity.data}
              onSubmit={handleSave}
              submitting={saving}
              submitLabel="Save"
              onCancel={handleCancelEdit}
              defaultJsonMode={editJsonMode || !entityTypeDef}
            />
          ) : (
            <pre className="max-h-[32rem] overflow-auto rounded-md bg-muted p-4 text-xs">
              {JSON.stringify(entity.data, null, 2)}
            </pre>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Relations</CardTitle>
          <CardDescription>
            {relations.length} relation{relations.length === 1 ? "" : "s"}
          </CardDescription>
        </CardHeader>
        <CardContent>
          {!context ? (
            <div className="space-y-2">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          ) : (
            <div className="overflow-x-auto rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Relation</TableHead>
                    <TableHead>Direction</TableHead>
                    <TableHead>Neighbor</TableHead>
                    <TableHead>Neighbor Type</TableHead>
                    <TableHead>Hop Distance</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {relations.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={5} className="text-center text-sm text-muted-foreground">
                        No relations found.
                      </TableCell>
                    </TableRow>
                  ) : (
                    relations.map((relation, index) => (
                      <TableRow key={`${relation.relation_type}-${relation.neighbor.id}-${index}`}>
                        <TableCell className="font-mono text-xs">
                          {relation.relation_type}
                        </TableCell>
                        <TableCell>
                          <Badge variant={relation.direction === "out" ? "default" : "outline"}>
                            {relation.direction}
                          </Badge>
                        </TableCell>
                        <TableCell>
                          <Link
                            to={`/ws/${wsId}/entities/${relation.neighbor.id}`}
                            className="font-mono text-xs text-link hover:underline"
                          >
                            {truncateId(relation.neighbor.id)}
                          </Link>
                        </TableCell>
                        <TableCell className="text-sm text-muted-foreground">
                          {relation.neighbor.entity_type}
                        </TableCell>
                        <TableCell className="text-sm text-muted-foreground">
                          {relation.hop_distance}
                        </TableCell>
                      </TableRow>
                    ))
                  )}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>

      <Dialog
        open={deleteOpen}
        onClose={() => {
          if (!deleting) setDeleteOpen(false);
        }}
        title="Delete Entity"
      >
        <div className="space-y-4">
          {deleteError && (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {deleteError}
            </div>
          )}
          <p className="text-sm text-foreground">
            Are you sure you want to delete this entity? This action cannot be undone.
          </p>
          <p
            className="break-all rounded-md bg-muted px-3 py-2 font-mono text-xs"
            title={entity.id}
          >
            {truncateId(entity.id)}
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setDeleteOpen(false)} disabled={deleting}>
              Cancel
            </Button>
            <Button variant="destructive" onClick={handleDelete} disabled={deleting}>
              {deleting ? "Deleting…" : "Delete"}
            </Button>
          </div>
        </div>
      </Dialog>
    </div>
  );
}

function BackLink({ wsId }: { wsId: string | undefined }) {
  return (
    <Link
      to={`/ws/${wsId}/entities`}
      className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
    >
      <ArrowLeft className="h-4 w-4" />
      Back to Entities
    </Link>
  );
}
