import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { listSchemas, createEntity, getActiveSchema } from "@/lib/api";
import type { Schema, SchemaDetail } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { EntityForm } from "@/components/entities/EntityForm";
import { useWorkspace } from "@/hooks/useWorkspace";
import { cn } from "@/lib/cn";

export function EntityCreatePage() {
  const navigate = useNavigate();
  const { workspaceId } = useWorkspace();

  const [schemas, setSchemas] = useState<Schema[]>([]);
  const [schemaName, setSchemaName] = useState("");
  const [schemaDetail, setSchemaDetail] = useState<SchemaDetail | null>(null);
  const [schemaLoading, setSchemaLoading] = useState(false);
  const [entityType, setEntityType] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    listSchemas()
      .then((list) => {
        if (cancelled) return;
        const active = list.filter((s) => s.status === "active");
        setSchemas(active);
        if (active.length === 1) setSchemaName(active[0].name);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!schemaName) {
      setSchemaDetail(null);
      setEntityType("");
      return;
    }
    let cancelled = false;
    setSchemaLoading(true);
    getActiveSchema(schemaName)
      .then((detail) => {
        if (cancelled) return;
        setSchemaDetail(detail);
        const types = Object.keys(detail.definition.entity_types ?? {});
        setEntityType(types[0] ?? "");
      })
      .catch((err) => {
        if (cancelled) return;
        setFormError(err instanceof Error ? err.message : "Failed to load schema");
      })
      .finally(() => {
        if (!cancelled) setSchemaLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [schemaName]);

  const entityTypeOptions = useMemo(
    () => Object.keys(schemaDetail?.definition.entity_types ?? {}),
    [schemaDetail],
  );

  const entityTypeDef = useMemo(
    () => (entityType ? schemaDetail?.definition.entity_types[entityType] : null),
    [schemaDetail, entityType],
  );

  async function handleSubmit(data: Record<string, unknown>) {
    setFormError(null);
    if (!schemaName || !entityType) {
      setFormError("Please select a schema and entity type.");
      return;
    }
    setSubmitting(true);
    try {
      const entity = await createEntity(schemaName, entityType, data);
      navigate(`/ws/${workspaceId}/entities/${entity.id}`);
    } catch (err) {
      setFormError(err instanceof Error ? err.message : "Failed to create entity");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="mx-auto max-w-2xl space-y-6 p-6">
      <div className="flex items-center gap-3">
        <Button variant="ghost" size="sm" onClick={() => navigate(`/ws/${workspaceId}/entities`)}>
          <ArrowLeft className="mr-1 h-4 w-4" />
          Back
        </Button>
        <h1 className="text-2xl font-semibold tracking-tight">Create Entity</h1>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base font-medium">Entity Details</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          {formError && (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {formError}
            </div>
          )}

          <div>
            <label
              htmlFor="schema-select"
              className="mb-1 block text-sm font-medium text-foreground"
            >
              Schema
            </label>
            <select
              id="schema-select"
              value={schemaName}
              onChange={(e) => setSchemaName(e.target.value)}
              className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring"
            >
              <option value="">Select a schema...</option>
              {schemas.map((schema) => (
                <option key={schema.id} value={schema.name}>
                  {schema.name} (v{schema.version})
                </option>
              ))}
            </select>
          </div>

          <div>
            <label
              htmlFor="entity-type-select"
              className="mb-1 block text-sm font-medium text-foreground"
            >
              Entity Type
            </label>
            <select
              id="entity-type-select"
              value={entityType}
              onChange={(e) => setEntityType(e.target.value)}
              disabled={!schemaName || schemaLoading || entityTypeOptions.length === 0}
              className={cn(
                "w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground shadow-sm focus:border-ring focus:outline-none focus:ring-2 focus:ring-ring",
                "disabled:cursor-not-allowed disabled:bg-secondary disabled:text-muted-foreground",
              )}
            >
              {entityTypeOptions.length === 0 && (
                <option value="">
                  {schemaLoading ? "Loading..." : "No entity types available"}
                </option>
              )}
              {entityTypeOptions.map((type) => (
                <option key={type} value={type}>
                  {type}
                </option>
              ))}
            </select>
          </div>

          {entityTypeDef && (
            <EntityForm
              key={`${schemaName}-${entityType}`}
              entityTypeDef={entityTypeDef}
              onSubmit={handleSubmit}
              submitting={submitting}
            />
          )}
        </CardContent>
      </Card>
    </div>
  );
}
