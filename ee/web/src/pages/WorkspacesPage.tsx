import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { Input } from "@/components/ui/Input";
import { Badge } from "@/components/ui/Badge";
import { PageSkeleton } from "@/components/ui/Skeleton";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/Table";
import { useWorkspace } from "@/hooks/useWorkspace";
import { listWorkspaces, listSchemas, createWorkspace } from "@/lib/api";
import type { Workspace, Schema } from "@/types/api";
import { formatDate } from "@/lib/format";

export function WorkspacesPage() {
  const { selectWorkspace } = useWorkspace();

  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [schemas, setSchemas] = useState<Schema[]>([]);
  const [schemasLoading, setSchemasLoading] = useState(false);
  const [schemasError, setSchemasError] = useState<string | null>(null);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [name, setName] = useState("");
  const [schemaId, setSchemaId] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  async function loadWorkspaces() {
    setLoading(true);
    setError(null);
    try {
      const data = await listWorkspaces();
      setWorkspaces(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load workspaces");
    } finally {
      setLoading(false);
    }
  }

  async function loadSchemas() {
    setSchemasLoading(true);
    setSchemasError(null);
    try {
      const data = await listSchemas();
      setSchemas(data);
      if (data.length > 0) {
        setSchemaId((prev) => prev || data[0].id);
      }
    } catch (err) {
      setSchemasError(err instanceof Error ? err.message : "Failed to load schemas");
    } finally {
      setSchemasLoading(false);
    }
  }

  useEffect(() => {
    loadWorkspaces();
    loadSchemas();
  }, []);

  function schemaName(id: string): string | null {
    return schemas.find((s) => s.id === id)?.name ?? null;
  }

  function openDialog() {
    setSubmitError(null);
    setName("");
    setDialogOpen(true);
    if (schemas.length === 0 && !schemasLoading) {
      loadSchemas();
    }
  }

  function closeDialog() {
    if (submitting) return;
    setDialogOpen(false);
  }

  function handleSelect(ws: Workspace) {
    selectWorkspace(ws.id, ws.name);
  }

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setSubmitError(null);

    if (!name.trim()) {
      setSubmitError("Name is required");
      return;
    }
    if (!schemaId) {
      setSubmitError("Schema is required");
      return;
    }

    setSubmitting(true);
    try {
      await createWorkspace(name.trim(), schemaId);
      setDialogOpen(false);
      setName("");
      await loadWorkspaces();
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : "Failed to create workspace");
    } finally {
      setSubmitting(false);
    }
  }

  if (loading) {
    return <PageSkeleton />;
  }

  return (
    <div className="space-y-6 p-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Workspaces</h1>
          <p className="text-sm text-muted-foreground">Select or create a workspace.</p>
        </div>
        <Button onClick={openDialog}>
          <Plus className="mr-2 h-4 w-4" />
          Create Workspace
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">All Workspaces</CardTitle>
          <CardDescription>Every workspace in your organization.</CardDescription>
        </CardHeader>
        <CardContent>
          {error ? (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive">
              {error}
            </div>
          ) : workspaces.length === 0 ? (
            <div className="py-10 text-center text-sm text-muted-foreground">
              No workspaces yet. Create one to get started.
            </div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Schema</TableHead>
                    <TableHead>Max Entities</TableHead>
                    <TableHead>Created</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {workspaces.map((ws) => (
                    <TableRow key={ws.id}>
                      <TableCell>
                        <button
                          type="button"
                          onClick={() => handleSelect(ws)}
                          className="font-medium text-link hover:underline"
                        >
                          {ws.name}
                        </button>
                      </TableCell>
                      <TableCell>
                        {ws.schema_id == null ? (
                          <span className="text-muted-foreground">—</span>
                        ) : schemaName(ws.schema_id) ? (
                          <Badge variant="secondary">{schemaName(ws.schema_id)}</Badge>
                        ) : (
                          // `schemas` only ever holds the signed-in workspace's schemas, so
                          // every other row with a schema lands here however healthy it is.
                          // Showing the id keeps the cell honest.
                          <span
                            className="text-muted-foreground font-mono text-xs"
                            title={`Schema ${ws.schema_id}: names resolve only for the workspace you are signed in to`}
                          >
                            {ws.schema_id.slice(0, 8)}
                          </span>
                        )}
                      </TableCell>
                      <TableCell className="text-muted-foreground">
                        {ws.max_entities === null ? "Unlimited" : ws.max_entities}
                      </TableCell>
                      <TableCell className="text-muted-foreground">
                        {formatDate(ws.created_at)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>

      <Dialog open={dialogOpen} onClose={closeDialog} title="Create Workspace" className="max-w-lg">
        <form onSubmit={handleSubmit} className="space-y-4">
          {submitError && (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              {submitError}
            </div>
          )}

          <Input
            label="Name"
            name="name"
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="my-workspace"
            required
            autoFocus
          />

          <div className="w-full">
            <label
              htmlFor="schema-select"
              className="mb-1 block text-sm font-medium text-foreground"
            >
              Schema
            </label>
            {schemasLoading ? (
              <div className="h-10 w-full animate-pulse rounded-md bg-muted" />
            ) : schemasError ? (
              <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
                {schemasError}
              </div>
            ) : schemas.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No schemas available. Create a schema before creating a workspace.
              </p>
            ) : (
              <select
                id="schema-select"
                value={schemaId}
                onChange={(event) => setSchemaId(event.target.value)}
                required
                className="w-full rounded-md border border-input px-3 py-2 text-sm text-foreground shadow-sm focus:outline-none focus:ring-2 focus:ring-ring focus:border-ring"
              >
                {schemas.map((schema) => (
                  <option key={schema.id} value={schema.id}>
                    {schema.name}
                  </option>
                ))}
              </select>
            )}
          </div>

          <div className="flex justify-end gap-2 pt-2">
            <Button type="button" variant="secondary" onClick={closeDialog} disabled={submitting}>
              Cancel
            </Button>
            <Button type="submit" disabled={submitting || schemasLoading || schemas.length === 0}>
              {submitting ? "Creating…" : "Create Workspace"}
            </Button>
          </div>
        </form>
      </Dialog>
    </div>
  );
}
