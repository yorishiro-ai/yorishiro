import type { SchemaDefinition } from "@/types/api";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/Card";
import { Badge } from "@/components/ui/Badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/Table";

/**
 * The "Entity Types" and "Relation Types" cards, rendered identically by the schema detail view
 * and the workspace schema view.
 *
 * `TemplateDetailPage` renders its own near-copy and is deliberately not a caller: it omits both
 * empty-state messages and hides the relation card entirely when a template defines no relations,
 * where these two always show the card with an explanatory row. Routing it through here would
 * change what that page displays for an empty template.
 */
export function SchemaDefinitionTables({ definition }: { definition: SchemaDefinition }) {
  const entityTypes = Object.entries(definition.entity_types ?? {});
  const relationTypes = Object.entries(definition.relation_types ?? {});

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Entity Types</CardTitle>
          <CardDescription>
            {entityTypes.length} entity type{entityTypes.length === 1 ? "" : "s"}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {entityTypes.length === 0 && (
            <p className="text-sm text-muted-foreground">No entity types defined.</p>
          )}
          {entityTypes.map(([entityName, entityDef]) => {
            const fields = Object.entries(entityDef.fields ?? {});
            return (
              <div key={entityName} className="space-y-2">
                <div className="flex items-baseline gap-2">
                  <h3 className="text-sm font-semibold">{entityName}</h3>
                  {entityDef.description && (
                    <span className="text-xs text-muted-foreground">{entityDef.description}</span>
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
                      {fields.length === 0 ? (
                        <TableRow>
                          <TableCell
                            colSpan={5}
                            className="text-center text-sm text-muted-foreground"
                          >
                            No fields defined.
                          </TableCell>
                        </TableRow>
                      ) : (
                        fields.map(([fieldName, fieldDef]) => (
                          <TableRow key={fieldName}>
                            <TableCell className="font-mono text-xs">{fieldName}</TableCell>
                            <TableCell className="font-mono text-xs">{fieldDef.type}</TableCell>
                            <TableCell>
                              {fieldDef.required ? (
                                <Badge variant="default">Required</Badge>
                              ) : (
                                <Badge variant="secondary">Optional</Badge>
                              )}
                            </TableCell>
                            <TableCell>
                              {fieldDef["x-embed"] ? (
                                <Badge variant="outline">x-embed</Badge>
                              ) : (
                                <span className="text-xs text-muted-foreground">—</span>
                              )}
                            </TableCell>
                            <TableCell className="text-sm text-muted-foreground">
                              {fieldDef.description ?? "—"}
                            </TableCell>
                          </TableRow>
                        ))
                      )}
                    </TableBody>
                  </Table>
                </div>
              </div>
            );
          })}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Relation Types</CardTitle>
          <CardDescription>
            {relationTypes.length} relation type{relationTypes.length === 1 ? "" : "s"}
          </CardDescription>
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
                {relationTypes.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={4} className="text-center text-sm text-muted-foreground">
                      No relation types defined.
                    </TableCell>
                  </TableRow>
                ) : (
                  relationTypes.map(([relationName, relationDef]) => (
                    <TableRow key={relationName}>
                      <TableCell className="font-mono text-xs">{relationName}</TableCell>
                      <TableCell className="font-mono text-xs">{relationDef.source}</TableCell>
                      <TableCell className="font-mono text-xs">{relationDef.target}</TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {relationDef.description ?? "—"}
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        </CardContent>
      </Card>
    </>
  );
}
