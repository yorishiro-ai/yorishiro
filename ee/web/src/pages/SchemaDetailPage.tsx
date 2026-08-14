import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, ChevronDown, ChevronRight } from "lucide-react";
import { getSchemaById } from "@/lib/api";
import type { SchemaDetail } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { formatDateTime } from "@/lib/format";
import { StatusBadge } from "@/components/ui/StatusBadge";
import { SchemaDefinitionTables } from "@/components/schema/SchemaDefinitionTables";
import { SchemaStructureCard } from "@/components/schema/SchemaStructureCard";

export function SchemaDetailPage() {
  const { schemaId } = useParams<{ schemaId: string }>();
  const [schema, setSchema] = useState<SchemaDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [rawOpen, setRawOpen] = useState(false);

  useEffect(() => {
    if (!schemaId) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    getSchemaById(schemaId)
      .then((data) => {
        if (!cancelled) setSchema(data);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load schema");
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [schemaId]);

  if (loading) {
    return <PageSkeleton />;
  }

  if (error || !schema) {
    return (
      <div className="space-y-4 p-6">
        <BackLink />
        <Card>
          <CardContent className="p-6">
            <p className="text-sm text-destructive">{error ?? "Schema not found."}</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  const { definition } = schema;

  return (
    <div className="space-y-6 p-6">
      <BackLink />

      <Card>
        <CardHeader>
          <div className="flex items-center gap-3">
            <CardTitle>{schema.name}</CardTitle>
            <StatusBadge status={schema.status} />
          </div>
          {definition.description && <CardDescription>{definition.description}</CardDescription>}
        </CardHeader>
        <CardContent>
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

      <SchemaStructureCard definition={definition} />

      <SchemaDefinitionTables definition={definition} />

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
              {JSON.stringify(definition, null, 2)}
            </pre>
          </CardContent>
        )}
      </Card>
    </div>
  );
}

function BackLink() {
  const navigate = useNavigate();
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      onClick={() => navigate("/schemas")}
      className="gap-1"
    >
      <ArrowLeft className="h-4 w-4" />
      Back to Schemas
    </Button>
  );
}
