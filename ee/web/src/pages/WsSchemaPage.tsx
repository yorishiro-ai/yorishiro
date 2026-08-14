import type { ChangeEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { ChevronDown, ChevronRight, Download, Upload, Pencil } from "lucide-react";
import { useWorkspace } from "@/hooks/useWorkspace";
import { cn } from "@/lib/cn";
import {
  getWorkspace,
  listSchemas,
  getSchemaById,
  createSchema,
  exportJsonl,
  importJsonl,
} from "@/lib/api";
import type { Schema, SchemaDetail, ImportResult } from "@/types/api";
import { FormJsonToggle } from "@/components/ui/FormJsonToggle";
import { SchemaStructureCard } from "@/components/schema/SchemaStructureCard";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { formatDateTime } from "@/lib/format";
import { StatusBadge } from "@/components/ui/StatusBadge";
import { SchemaDefinitionTables } from "@/components/schema/SchemaDefinitionTables";
import { SchemaVersionDiff } from "@/components/schema/SchemaVersionDiff";
import { unzipJsonl } from "@/lib/unzipJsonl";

function downloadJsonl(content: string, filename: string) {
  const blob = new Blob([content], { type: "application/x-ndjson" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}

function timestampedFilename(): string {
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(
    now.getHours(),
  )}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `yorishiro-export-${stamp}.jsonl`;
}

export function WsSchemaPage() {
  const { workspaceId } = useWorkspace();
  const location = useLocation();
  const isIoTab = location.pathname.endsWith("/io");

  const [versions, setVersions] = useState<Schema[]>([]);
  const [selectedSchemaId, setSelectedSchemaId] = useState<string>("");
  const [schema, setSchema] = useState<SchemaDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [detailLoading, setDetailLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [lastExportFilename, setLastExportFilename] = useState<string | null>(null);

  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [importResult, setImportResult] = useState<ImportResult | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  async function loadVersions() {
    if (!workspaceId) return;
    setLoading(true);
    setError(null);
    setSchema(null);
    try {
      const workspace = await getWorkspace(workspaceId);
      const allSchemas = await listSchemas();

      let match = workspace.schema_id ? allSchemas.find((s) => s.id === workspace.schema_id) : null;

      if (!match) {
        const active = allSchemas.filter((s) => s.status === "active");
        if (active.length > 0) match = active[0];
      }

      if (!match) {
        setError("No active schema found for this workspace.");
        return;
      }

      const sameName = allSchemas
        .filter((s) => s.name === match!.name)
        .toSorted((a, b) => b.version - a.version);
      setVersions(sameName);

      const activeVersion = sameName.find((s) => s.status === "active") ?? sameName[0];
      setSelectedSchemaId(activeVersion.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load schema");
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    if (!workspaceId) {
      setLoading(false);
      return;
    }
    loadVersions();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId]);

  useEffect(() => {
    if (!selectedSchemaId) return;
    let cancelled = false;
    setError(null);
    setDetailLoading(true);
    getSchemaById(selectedSchemaId)
      .then((detail) => {
        if (!cancelled) setSchema(detail);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load schema");
      })
      .finally(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedSchemaId]);

  async function loadSchema() {
    await loadVersions();
  }

  async function handleExport() {
    setExporting(true);
    setExportError(null);
    try {
      const content = await exportJsonl();
      const filename = timestampedFilename();
      downloadJsonl(content, filename);
      setLastExportFilename(filename);
    } catch (err) {
      setExportError(err instanceof Error ? err.message : "Failed to export");
    } finally {
      setExporting(false);
    }
  }

  function handleFileChange(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0] ?? null;
    setSelectedFile(file);
    setImportError(null);
    setImportResult(null);
  }

  async function handleImport() {
    if (!selectedFile) {
      setImportError("Choose a .jsonl or .zip file to import");
      return;
    }
    setImporting(true);
    setImportError(null);
    setImportResult(null);
    try {
      const isZip = selectedFile.name.toLowerCase().endsWith(".zip");
      let result: ImportResult;

      if (isZip) {
        const entries = unzipJsonl(new Uint8Array(await selectedFile.arrayBuffer()));
        if (entries.length === 0) {
          throw new Error("The archive contains no .jsonl files");
        }
        // Imported one member at a time, in name order, so that a file defining schemas can be
        // applied before the entities that reference them. Totals are summed across members.
        result = { schemas: 0, entities: 0, relations: 0, errors: [] };
        for (const entry of entries) {
          try {
            const partial = await importJsonl(entry.content);
            result.schemas += partial.schemas;
            result.entities += partial.entities;
            result.relations += partial.relations;
            // A 200 carrying errors means that member was rolled back, so its counts were
            // never committed -- surface it as a failure rather than adding zeroes silently.
            if (partial.errors.length > 0) {
              throw new Error(partial.errors.join("; "));
            }
          } catch (err) {
            // Name the member that failed: without it the error reads as if the whole archive
            // was malformed, and earlier members have already been imported.
            const message = err instanceof Error ? err.message : "Failed to import";
            throw new Error(`${entry.name}: ${message}`);
          }
        }
      } else {
        result = await importJsonl(await selectedFile.text());
      }

      setImportResult(result);
      setSelectedFile(null);
      if (fileInputRef.current) fileInputRef.current.value = "";
    } catch (err) {
      setImportError(err instanceof Error ? err.message : "Failed to import");
    } finally {
      setImporting(false);
    }
  }

  if (!workspaceId) {
    return (
      <div className="space-y-4 p-6">
        <Card>
          <CardContent className="p-6">
            <p className="text-sm text-muted-foreground">No workspace selected.</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (loading) return <PageSkeleton />;

  if (error) {
    return (
      <div className="space-y-4 p-6">
        <Card>
          <CardContent className="p-6">
            <p className="text-sm text-destructive">{error}</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!schema) return <PageSkeleton />;

  const isReadOnly = schema.status !== "active";

  const schemaBase = `/ws/${workspaceId}/schema`;

  return (
    <div className="space-y-6 p-6">
      {/* Schema summary */}
      <Card>
        <CardHeader>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-3">
              <CardTitle>{schema.name}</CardTitle>
              <StatusBadge status={schema.status} />
            </div>
            <div className="flex items-center gap-2">
              <label
                htmlFor="schema-version-select"
                className="text-xs font-medium text-muted-foreground"
              >
                Version
              </label>
              <select
                id="schema-version-select"
                value={selectedSchemaId}
                onChange={(e) => setSelectedSchemaId(e.target.value)}
                disabled={detailLoading}
                className="h-9 rounded-md border border-input bg-card px-3 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring disabled:opacity-60"
              >
                {versions.map((v) => (
                  <option key={v.id} value={v.id}>
                    v{v.version} ({v.status})
                  </option>
                ))}
              </select>
            </div>
          </div>
          <CardDescription>Schema definition for this workspace.</CardDescription>
        </CardHeader>
        <CardContent>
          {isReadOnly && (
            <div className="mb-4 rounded-md border border-border bg-secondary px-3 py-2 text-sm text-secondary-foreground">
              This version is read-only. Switch to the active version to edit the definition or
              import/export data.
            </div>
          )}
          <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Name</dt>
              <dd className="text-sm">{schema.name}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Version</dt>
              <dd className="text-sm">{schema.version}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Status</dt>
              <dd className="text-sm">
                <StatusBadge status={schema.status} />
              </dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-muted-foreground">Created At</dt>
              <dd className="text-sm">{formatDateTime(schema.created_at)}</dd>
            </div>
          </dl>
        </CardContent>
      </Card>

      {/* Tab navigation */}
      <div className="flex gap-1 border-b border-border">
        <NavLink
          to={schemaBase}
          end
          className={({ isActive }) =>
            cn(
              "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors",
              isActive
                ? "border-primary text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )
          }
        >
          Definition
        </NavLink>
        <NavLink
          to={isReadOnly ? schemaBase : `${schemaBase}/io`}
          aria-disabled={isReadOnly}
          onClick={(e) => {
            if (isReadOnly) e.preventDefault();
          }}
          className={({ isActive }) =>
            cn(
              "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors",
              isReadOnly
                ? "cursor-not-allowed border-transparent text-muted-foreground/50"
                : isActive
                  ? "border-primary text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground",
            )
          }
        >
          Import / Export
        </NavLink>
      </div>

      {isIoTab ? (
        isReadOnly ? (
          <Card>
            <CardContent className="p-6">
              <p className="text-sm text-muted-foreground">
                Import/Export is not available for archived schema versions. Switch to the active
                version to import or export data.
              </p>
            </CardContent>
          </Card>
        ) : (
          <IoSection
            exporting={exporting}
            exportError={exportError}
            lastExportFilename={lastExportFilename}
            onExport={handleExport}
            importing={importing}
            importError={importError}
            importResult={importResult}
            selectedFile={selectedFile}
            fileInputRef={fileInputRef}
            onFileChange={handleFileChange}
            onImport={handleImport}
          />
        )
      ) : (
        <DefinitionSection
          schema={schema}
          versions={versions}
          isReadOnly={isReadOnly}
          onSchemaUpdated={loadSchema}
        />
      )}
    </div>
  );
}

function DefinitionSection({
  schema,
  versions,
  isReadOnly,
  onSchemaUpdated,
}: {
  schema: SchemaDetail;
  versions: Schema[];
  isReadOnly: boolean;
  onSchemaUpdated: () => void;
}) {
  const { definition } = schema;
  const [rawOpen, setRawOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveSuccess, setSaveSuccess] = useState<string | null>(null);

  async function handleSaveNewVersion(data: Record<string, unknown>) {
    setSaving(true);
    setSaveError(null);
    setSaveSuccess(null);
    try {
      const { schema: newSchema } = await createSchema(data);
      setSaveSuccess(`New version v${newSchema.version} created.`);
      setEditing(false);
      onSchemaUpdated();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : "Failed to create new version");
    } finally {
      setSaving(false);
    }
  }

  if (editing && !isReadOnly) {
    return (
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Edit Schema Definition</CardTitle>
          <CardDescription>
            Editing will create a new version (v{schema.version + 1}). The current version will be
            archived.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {saveError && (
            <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {saveError}
            </div>
          )}
          <FormJsonToggle
            data={definition as unknown as Record<string, unknown>}
            onSubmit={handleSaveNewVersion}
            submitting={saving}
            submitLabel={`Create v${schema.version + 1}`}
            onCancel={() => {
              setEditing(false);
              setSaveError(null);
            }}
            defaultJsonMode
          >
            {() => (
              <p className="text-sm text-muted-foreground">
                Use JSON Mode to edit the full schema definition.
              </p>
            )}
          </FormJsonToggle>
        </CardContent>
      </Card>
    );
  }

  return (
    <>
      {saveSuccess && (
        <div className="rounded-md border border-primary/30 bg-primary/10 p-3 text-sm text-foreground">
          {saveSuccess}
        </div>
      )}

      {/* Edit button */}
      {!isReadOnly && (
        <div className="flex justify-end">
          <Button variant="secondary" size="sm" onClick={() => setEditing(true)}>
            <Pencil className="mr-1 h-3 w-3" />
            Edit Definition
          </Button>
        </div>
      )}

      <SchemaStructureCard definition={definition} />

      <SchemaDefinitionTables definition={definition} />

      <SchemaVersionDiff versions={versions} />

      {/* Raw JSON */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle className="text-lg">Raw JSON</CardTitle>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={() => setRawOpen((prev) => !prev)}
              aria-expanded={rawOpen}
              className="gap-1"
            >
              {rawOpen ? (
                <ChevronDown className="h-4 w-4 shrink-0" />
              ) : (
                <ChevronRight className="h-4 w-4 shrink-0" />
              )}
              {rawOpen ? "Hide" : "Show"}
            </Button>
          </div>
          <CardDescription>Full schema definition as stored.</CardDescription>
        </CardHeader>
        {rawOpen && (
          <CardContent>
            <pre className="max-h-[32rem] overflow-auto rounded-md bg-muted p-4 text-xs">
              {JSON.stringify(schema.definition, null, 2)}
            </pre>
          </CardContent>
        )}
      </Card>
    </>
  );
}

interface IoSectionProps {
  exporting: boolean;
  exportError: string | null;
  lastExportFilename: string | null;
  onExport: () => void;
  importing: boolean;
  importError: string | null;
  importResult: ImportResult | null;
  selectedFile: File | null;
  fileInputRef: React.RefObject<HTMLInputElement | null>;
  onFileChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onImport: () => void;
}

function IoSection(props: IoSectionProps) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-lg">Import / Export</CardTitle>
        <CardDescription>Import and export workspace data as JSON Lines.</CardDescription>
      </CardHeader>
      <CardContent>
        <div className="grid gap-6 sm:grid-cols-2">
          <div className="space-y-3">
            <h4 className="flex items-center gap-2 text-sm font-medium">
              <Download className="h-4 w-4" />
              Export
            </h4>
            <p className="text-xs text-muted-foreground">
              Download all schemas, entities, and relations as a .jsonl file.
            </p>
            {props.exportError && (
              <div className="rounded-md border border-destructive/30 bg-destructive/10 p-2 text-xs text-destructive">
                {props.exportError}
              </div>
            )}
            {props.lastExportFilename && !props.exportError && (
              <div className="rounded-md border border-border bg-secondary p-2 text-xs text-secondary-foreground">
                Downloaded {props.lastExportFilename}
              </div>
            )}
            <Button size="sm" onClick={props.onExport} disabled={props.exporting}>
              {props.exporting ? "Exporting..." : "Export JSONL"}
            </Button>
          </div>

          <div className="space-y-3">
            <h4 className="flex items-center gap-2 text-sm font-medium">
              <Upload className="h-4 w-4" />
              Import
            </h4>
            <p className="text-xs text-muted-foreground">
              Upload a .jsonl file to import schemas, entities, and relations, or a .zip archive
              containing several. Archived members are imported in name order.
            </p>
            {props.importError && (
              <div className="rounded-md border border-destructive/30 bg-destructive/10 p-2 text-xs text-destructive">
                {props.importError}
              </div>
            )}
            {props.importResult &&
              (props.importResult.errors.length > 0 ? (
                <div className="rounded-md border border-destructive/30 bg-destructive/10 p-2 text-xs text-destructive">
                  Import rolled back, nothing was applied: {props.importResult.errors.join("; ")}
                </div>
              ) : (
                <div className="rounded-md border border-border bg-secondary p-2 text-xs text-secondary-foreground">
                  Imported: {props.importResult.schemas} schemas, {props.importResult.entities}{" "}
                  entities, {props.importResult.relations} relations
                </div>
              ))}
            <Input
              ref={props.fileInputRef}
              type="file"
              accept=".jsonl,.zip,application/x-ndjson,application/zip"
              onChange={props.onFileChange}
            />
            <Button
              size="sm"
              onClick={props.onImport}
              disabled={props.importing || !props.selectedFile}
            >
              {props.importing ? "Importing..." : "Import"}
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
