import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Plus, LayoutTemplate, Eye } from "lucide-react";
import { ReactFlow, ReactFlowProvider, Background, Controls } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Dialog } from "@/components/ui/Dialog";
import { Badge } from "@/components/ui/Badge";
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
import { schemaNodeTypes, useSchemaGraph } from "@/components/graph/SchemaGraph";
import { cn } from "@/lib/cn";
import {
  listSchemas,
  createSchema,
  createSchemaFromTemplate,
  listTemplates,
  getTemplate,
} from "@/lib/api";
import type { Schema, SchemaDefinition, Template } from "@/types/api";
import { formatDate } from "@/lib/format";
import { useIsDarkMode } from "@/hooks/useIsDarkMode";

type PageTab = "all" | "templates";

function statusVariant(status: string): "default" | "secondary" | "outline" | "destructive" {
  switch (status.toLowerCase()) {
    case "active":
      return "default";
    case "draft":
      return "secondary";
    case "deprecated":
      return "destructive";
    default:
      return "outline";
  }
}

// ── Page ─────────────────────────────────────────────────────────────────

export function SchemasPage() {
  const [tab, setTab] = useState<PageTab>("all");

  const [schemas, setSchemas] = useState<Schema[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [customName, setCustomName] = useState("");
  const [customDefinition, setCustomDefinition] = useState("");
  const [customJsonError, setCustomJsonError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  async function loadSchemas() {
    setLoading(true);
    setError(null);
    try {
      const data = await listSchemas();
      setSchemas(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load schemas");
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    loadSchemas();
  }, []);

  function openDialog() {
    setSubmitError(null);
    setCustomJsonError(null);
    setDialogOpen(true);
  }

  function closeDialog() {
    if (submitting) return;
    setDialogOpen(false);
  }

  async function handleCustomSubmit(event: FormEvent) {
    event.preventDefault();
    setCustomJsonError(null);
    setSubmitError(null);

    if (!customName.trim()) {
      setSubmitError("Name is required");
      return;
    }

    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(customDefinition);
    } catch {
      setCustomJsonError("Definition must be valid JSON");
      return;
    }

    const definition = { name: customName.trim(), ...parsed };

    setSubmitting(true);
    try {
      await createSchema(definition);
      setDialogOpen(false);
      setCustomName("");
      setCustomDefinition("");
      await loadSchemas();
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : "Failed to create schema");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="space-y-6 p-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Schemas</h1>
          <p className="text-sm text-muted-foreground">
            Your organization's schemas. Schemas are shared across your tenant — each workspace is
            created against one of these schemas.
          </p>
        </div>
        {tab === "all" && (
          <Button onClick={openDialog}>
            <Plus className="mr-2 h-4 w-4" />
            Create Custom Schema
          </Button>
        )}
      </div>

      <div className="flex rounded-lg border border-border bg-secondary p-0.5 w-fit">
        <button
          type="button"
          onClick={() => setTab("all")}
          className={cn(
            "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
            tab === "all" ? "bg-card shadow-sm" : "text-muted-foreground hover:text-foreground",
          )}
        >
          All Schemas
        </button>
        <button
          type="button"
          onClick={() => setTab("templates")}
          className={cn(
            "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
            tab === "templates"
              ? "bg-card shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          Templates
        </button>
      </div>

      {tab === "all" ? (
        <AllSchemasTab schemas={schemas} loading={loading} error={error} />
      ) : (
        <TemplatesTab
          schemas={schemas}
          onSchemaCreated={async () => {
            await loadSchemas();
            setTab("all");
          }}
        />
      )}

      <Dialog
        open={dialogOpen}
        onClose={closeDialog}
        title="Create Custom Schema"
        className="max-w-lg"
      >
        {submitError && (
          <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            {submitError}
          </div>
        )}

        <form onSubmit={handleCustomSubmit} className="space-y-4">
          <Input
            label="Name"
            name="name"
            value={customName}
            onChange={(event) => setCustomName(event.target.value)}
            placeholder="my_schema"
            required
          />
          <div className="w-full">
            <label htmlFor="definition" className="mb-1 block text-sm font-medium text-foreground">
              Definition (JSON)
            </label>
            <textarea
              id="definition"
              name="definition"
              value={customDefinition}
              onChange={(event) => setCustomDefinition(event.target.value)}
              placeholder={'{\n  "entity_types": {},\n  "relation_types": {}\n}'}
              rows={10}
              className={cn(
                "w-full rounded-md border border-input px-3 py-2 font-mono text-xs text-foreground shadow-sm placeholder:text-muted-foreground",
                "focus:outline-none focus:ring-2 focus:ring-ring focus:border-ring",
                customJsonError &&
                  "border-destructive focus:ring-destructive focus:border-destructive",
              )}
              required
            />
            {customJsonError && <p className="mt-1 text-sm text-destructive">{customJsonError}</p>}
          </div>

          <div className="flex justify-end gap-2 pt-2">
            <Button type="button" variant="secondary" onClick={closeDialog} disabled={submitting}>
              Cancel
            </Button>
            <Button type="submit" disabled={submitting}>
              {submitting ? "Creating…" : "Create Schema"}
            </Button>
          </div>
        </form>
      </Dialog>
    </div>
  );
}

// ── Tab 1: All Schemas ──────────────────────────────────────────────────

function AllSchemasTab({
  schemas,
  loading,
  error,
}: {
  schemas: Schema[];
  loading: boolean;
  error: string | null;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">All Schemas</CardTitle>
        <p className="text-sm text-muted-foreground">
          Visible to every workspace in your organization.
        </p>
      </CardHeader>
      <CardContent>
        {loading ? (
          <div className="space-y-2">
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
            <Skeleton className="h-10 w-full" />
          </div>
        ) : error ? (
          <div className="rounded-md border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive">
            {error}
          </div>
        ) : schemas.length === 0 ? (
          <div className="py-10 text-center text-sm text-muted-foreground">
            No schemas yet. Create one to get started.
          </div>
        ) : (
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Version</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead>Created</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {schemas.map((schema) => (
                  <TableRow key={schema.id}>
                    <TableCell>
                      <Link
                        to={`/schemas/${schema.id}`}
                        className="font-medium text-link hover:underline"
                      >
                        {schema.name}
                      </Link>
                    </TableCell>
                    <TableCell>v{schema.version}</TableCell>
                    <TableCell>
                      <Badge variant={statusVariant(schema.status)}>{schema.status}</Badge>
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {formatDate(schema.created_at)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

// ── Tab 2: Templates ────────────────────────────────────────────────────

function TemplatesTab({
  schemas,
  onSchemaCreated,
}: {
  schemas: Schema[];
  onSchemaCreated: () => void | Promise<void>;
}) {
  const [templates, setTemplates] = useState<Template[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [applyingId, setApplyingId] = useState<string | null>(null);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [successMessage, setSuccessMessage] = useState<string | null>(null);

  const [previewDef, setPreviewDef] = useState<SchemaDefinition | null>(null);
  const [previewName, setPreviewName] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    listTemplates()
      .then((data) => {
        if (!cancelled) setTemplates(data);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load templates");
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const isDark = useIsDarkMode();
  const { nodes, edges } = useSchemaGraph(previewDef, isDark);

  async function handlePreview(template: Template) {
    if (previewName === template.name) {
      setPreviewDef(null);
      setPreviewName(null);
      return;
    }
    setPreviewLoading(true);
    try {
      const def = await getTemplate(template.id);
      setPreviewDef(def);
      setPreviewName(template.name);
    } catch {
      setPreviewDef(null);
      setPreviewName(null);
    } finally {
      setPreviewLoading(false);
    }
  }

  function isDuplicate(template: Template): boolean {
    return schemas.some((s) => s.name === template.name && s.status === "active");
  }

  async function handleApply(template: Template) {
    setApplyingId(template.id);
    setApplyError(null);
    setSuccessMessage(null);
    try {
      const { schema } = await createSchemaFromTemplate(template.id);
      setSuccessMessage(
        `Schema "${schema.name}" v${schema.version} created from "${template.name}".`,
      );
      await onSchemaCreated();
    } catch (err) {
      setApplyError(err instanceof Error ? err.message : "Failed to create schema from template");
    } finally {
      setApplyingId(null);
    }
  }

  if (loading) {
    return (
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Skeleton className="h-32 w-full" />
        <Skeleton className="h-32 w-full" />
        <Skeleton className="h-32 w-full" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-md border border-destructive/30 bg-destructive/10 p-4 text-sm text-destructive">
        {error}
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {applyError && (
        <div className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
          {applyError}
        </div>
      )}
      {successMessage && (
        <div className="rounded-md border border-primary/30 bg-primary/10 p-3 text-sm text-foreground">
          {successMessage}
        </div>
      )}

      {templates.length === 0 ? (
        <Card>
          <CardContent className="py-10 text-center text-sm text-muted-foreground">
            No templates available.
          </CardContent>
        </Card>
      ) : (
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {templates.map((template) => {
            const duplicate = isDuplicate(template);
            return (
              <Card
                key={template.id}
                className={cn(
                  "flex flex-col transition-colors",
                  previewName === template.name && "border-primary",
                )}
              >
                <CardHeader>
                  <div className="flex items-center gap-2">
                    <LayoutTemplate className="h-4 w-4 text-muted-foreground" />
                    <CardTitle className="text-base">
                      <Link to={`/schemas/templates/${template.id}`} className="hover:underline">
                        {template.name}
                      </Link>
                    </CardTitle>
                  </div>
                </CardHeader>
                <CardContent className="flex flex-1 flex-col justify-between gap-3">
                  <p className="text-sm text-muted-foreground">{template.description}</p>
                  {duplicate && (
                    <p className="text-xs text-amber-600 dark:text-amber-400">
                      A schema with this name already exists. Applying will create a new version.
                    </p>
                  )}
                  <div className="flex gap-2">
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={() => handlePreview(template)}
                      disabled={previewLoading}
                    >
                      <Eye className="mr-1 h-3 w-3" />
                      {previewName === template.name ? "Hide" : "Preview"}
                    </Button>
                    <Button
                      size="sm"
                      onClick={() => handleApply(template)}
                      disabled={applyingId !== null}
                    >
                      {applyingId === template.id
                        ? "Applying..."
                        : duplicate
                          ? "New version"
                          : "Apply"}
                    </Button>
                  </div>
                </CardContent>
              </Card>
            );
          })}
        </div>
      )}

      {previewDef && (
        <Card className="flex min-h-[420px] flex-col overflow-hidden">
          <CardHeader>
            <CardTitle className="text-base">Preview: {previewName}</CardTitle>
            <p className="text-sm text-muted-foreground">Schema structure before applying.</p>
          </CardHeader>
          <CardContent className="flex-1 p-0">
            {nodes.length === 0 ? (
              <div className="flex h-full items-center justify-center py-10 text-sm text-muted-foreground">
                No entity types to display.
              </div>
            ) : (
              <div className="h-[380px]">
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
                    <Controls />
                  </ReactFlow>
                </ReactFlowProvider>
              </div>
            )}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
