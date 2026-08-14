import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { getTemplate, listSchemas, createSchemaFromTemplate } from "@/lib/api";
import type { Schema, SchemaDefinition } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/Table";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { SchemaStructureCard } from "@/components/schema/SchemaStructureCard";

export function TemplateDetailPage() {
  const { id } = useParams<{ id: string }>();

  const [definition, setDefinition] = useState<SchemaDefinition | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [schemas, setSchemas] = useState<Schema[]>([]);
  const [applying, setApplying] = useState(false);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [applySuccess, setApplySuccess] = useState<string | null>(null);

  useEffect(() => {
    if (!id) return;
    let cancelled = false;
    setLoading(true);
    Promise.all([getTemplate(id), listSchemas()])
      .then(([def, schemaList]) => {
        if (cancelled) return;
        setDefinition(def);
        setSchemas(schemaList);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : "Failed to load template");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  const duplicate = definition
    ? schemas.some((s) => s.name === definition.name && s.status === "active")
    : false;

  async function handleApply() {
    if (!id) return;
    setApplying(true);
    setApplyError(null);
    setApplySuccess(null);
    try {
      const { schema } = await createSchemaFromTemplate(id);
      setApplySuccess(`Schema "${schema.name}" v${schema.version} created.`);
      const updated = await listSchemas();
      setSchemas(updated);
    } catch (err) {
      setApplyError(err instanceof Error ? err.message : "Failed to apply template");
    } finally {
      setApplying(false);
    }
  }

  if (loading) return <PageSkeleton />;

  if (error || !definition) {
    return (
      <div className="space-y-4 p-6">
        <BackLink />
        <Card>
          <CardContent className="p-6">
            <p className="text-sm text-destructive">{error ?? "Template not found."}</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  const entityTypes = Object.entries(definition.entity_types ?? {});
  const relationTypes = Object.entries(definition.relation_types ?? {});

  return (
    <div className="space-y-6 p-6">
      <BackLink />

      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle>{definition.name}</CardTitle>
              {definition.description && (
                <CardDescription>{definition.description}</CardDescription>
              )}
            </div>
            <div className="flex items-center gap-2">
              {duplicate && (
                <span className="text-xs text-amber-600 dark:text-amber-400">Already exists</span>
              )}
              <Button onClick={handleApply} disabled={applying}>
                {applying ? "Applying..." : duplicate ? "New Version" : "Apply Template"}
              </Button>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {applyError && (
            <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {applyError}
            </div>
          )}
          {applySuccess && (
            <div className="mb-4 rounded-md border border-primary/30 bg-primary/10 px-3 py-2 text-sm text-foreground">
              {applySuccess}
            </div>
          )}
        </CardContent>
      </Card>

      <SchemaStructureCard
        definition={definition}
        description="Visual structure of entity types and relations."
      />

      {/* Entity Types */}
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Entity Types</CardTitle>
          <CardDescription>
            {entityTypes.length} entity type{entityTypes.length === 1 ? "" : "s"}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {entityTypes.map(([name, def]) => {
            const fields = Object.entries(def.fields ?? {});
            return (
              <div key={name} className="space-y-2">
                <div className="flex items-baseline gap-2">
                  <h3 className="text-sm font-semibold">{name}</h3>
                  {def.description && (
                    <span className="text-xs text-muted-foreground">{def.description}</span>
                  )}
                </div>
                <div className="overflow-x-auto rounded-md border">
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Field</TableHead>
                        <TableHead>Type</TableHead>
                        <TableHead>Required</TableHead>
                        <TableHead>Embed</TableHead>
                        <TableHead>Description</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {fields.map(([fname, fdef]) => (
                        <TableRow key={fname}>
                          <TableCell className="font-mono text-xs">{fname}</TableCell>
                          <TableCell className="font-mono text-xs">{fdef.type}</TableCell>
                          <TableCell>
                            <Badge variant={fdef.required ? "default" : "secondary"}>
                              {fdef.required ? "Required" : "Optional"}
                            </Badge>
                          </TableCell>
                          <TableCell>
                            {fdef["x-embed"] ? (
                              <Badge variant="outline">x-embed</Badge>
                            ) : (
                              <span className="text-xs text-muted-foreground">—</span>
                            )}
                          </TableCell>
                          <TableCell className="text-sm text-muted-foreground">
                            {fdef.description ?? "—"}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </div>
              </div>
            );
          })}
        </CardContent>
      </Card>

      {/* Relation Types */}
      {relationTypes.length > 0 && (
        <Card>
          <CardHeader>
            <CardTitle className="text-lg">Relation Types</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="overflow-x-auto rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Relation</TableHead>
                    <TableHead>Source</TableHead>
                    <TableHead>Target</TableHead>
                    <TableHead>Description</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {relationTypes.map(([rname, rdef]) => (
                    <TableRow key={rname}>
                      <TableCell className="font-mono text-xs">{rname}</TableCell>
                      <TableCell className="font-mono text-xs">{rdef.source}</TableCell>
                      <TableCell className="font-mono text-xs">{rdef.target}</TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {rdef.description ?? "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Raw JSON */}
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Raw JSON</CardTitle>
        </CardHeader>
        <CardContent>
          <pre className="max-h-[32rem] overflow-auto rounded-md bg-muted p-4 text-xs">
            {JSON.stringify(definition, null, 2)}
          </pre>
        </CardContent>
      </Card>
    </div>
  );
}

function BackLink() {
  return (
    <a
      href="/schemas"
      className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      onClick={(e) => {
        e.preventDefault();
        history.back();
      }}
    >
      <ArrowLeft className="h-4 w-4" />
      Back to Schemas
    </a>
  );
}
