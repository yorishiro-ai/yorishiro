/**
 * Guards the failure mode that shipped twice in this codebase: a type in `types/api.ts` that is
 * internally consistent, compiles, passes `tsc`, and simply does not describe what the server
 * sends. `searchEntities` was declared `Promise<Entity[]>` against an endpoint returning
 * `SearchHit[]`, and `Relation` carried flat `neighbor_id`/`neighbor_type` fields the server has
 * never sent. Both rendered blank rows in production; neither is detectable by a type checker,
 * because nothing in the frontend disagrees with itself.
 *
 * The expectations here are not hand-written. They are read out of `__fixtures__/server-schemas.json`,
 * which is captured verbatim from the server's own OpenAPI document (`/api-docs/openapi.json`,
 * generated from the Rust types by utoipa). Hand-writing a fixture would just re-encode whatever
 * the author believed the shape was -- exactly the mistake being guarded against.
 *
 * Refresh the fixture after a base lockstep bump:
 *   curl -s localhost:8081/api-docs/openapi.json | jq '.components.schemas'
 */
import { describe, expect, it } from "vitest";
import schemas from "./__fixtures__/server-schemas.json";

type JsonSchema = {
  type?: string | string[];
  required?: string[];
  properties?: Record<string, JsonSchema & { $ref?: string }>;
  $ref?: string;
};

// The fixture is a captured OpenAPI document, and its inferred shape already satisfies
// `JsonSchema`, so the annotation carries the intent without an assertion to hide a mismatch.
const asSchema = (name: keyof typeof schemas): JsonSchema => schemas[name];

/** Property names the server declares for a schema, from the OpenAPI document. */
function serverFields(name: keyof typeof schemas): string[] {
  return Object.keys(asSchema(name).properties ?? {}).toSorted();
}

describe("search results", () => {
  it("wraps the entity rather than returning it bare", () => {
    // The exact bug: the page read result.id / result.entity_type / result.data off the top
    // level. Those live one level down, under `entity`.
    expect(serverFields("SearchHit")).toEqual(["distance", "entity"]);
    expect(asSchema("SearchHit").properties?.entity?.$ref).toContain("EntityRecord");
  });

  it("does not expose entity fields at the top level of a hit", () => {
    for (const leaked of ["id", "entity_type", "data"]) {
      expect(serverFields("SearchHit")).not.toContain(leaked);
    }
  });

  it("carries a distance, not a similarity -- smaller means closer", () => {
    // Guards the inverted-meaning trap: a `similarity` column built from this value without
    // converting it would rank results backwards.
    expect(serverFields("SearchHit")).toContain("distance");
    expect(serverFields("SearchHit")).not.toContain("similarity");
    expect(serverFields("SearchHit")).not.toContain("score");
  });
});

describe("entity context relations", () => {
  it("nests the neighbour entity instead of flattening its id and type", () => {
    // The second bug: `neighbor_id` / `neighbor_type` never existed on the wire.
    expect(serverFields("RecallRelation")).toEqual([
      "direction",
      "hop_distance",
      "neighbor",
      "relation_type",
    ]);
    expect(asSchema("RecallRelation").properties?.neighbor?.$ref).toContain("EntityRecord");
  });

  it("has no flattened neighbour fields", () => {
    for (const leaked of ["neighbor_id", "neighbor_type"]) {
      expect(serverFields("RecallRelation")).not.toContain(leaked);
    }
  });

  it("returns the context as { entity, relations, truncated }", () => {
    expect(serverFields("RecallContext")).toEqual(["entity", "relations", "truncated"]);
  });
});

describe("import results", () => {
  it("counts records with bare names, not _imported suffixes", () => {
    // The third instance of the same bug: `ImportResult` was declared with
    // `schemas_imported`/`entities_imported`/`relations_imported`, which the server has never
    // sent. A single-file import rendered "undefined schemas"; summing them across the members
    // of a ZIP rendered "NaN schemas".
    expect(serverFields("ImportResult")).toEqual(["entities", "errors", "relations", "schemas"]);
    for (const leaked of ["schemas_imported", "entities_imported", "relations_imported"]) {
      expect(serverFields("ImportResult")).not.toContain(leaked);
    }
  });

  it("reports rollback through errors, so a count alone does not mean success", () => {
    // A rolled-back import still answers 200 with counts; `errors` is what distinguishes it.
    expect(asSchema("ImportResult").required).toContain("errors");
  });
});

describe("entity records", () => {
  it("declares the fields the entity views read", () => {
    // These are read directly by EntitiesPage/EntityDetailPage; a rename upstream should fail
    // here rather than in a browser.
    const fields = serverFields("EntityRecord");
    for (const required of [
      "id",
      "workspace_id",
      "schema_id",
      "schema_version",
      "entity_type",
      "data",
      "created_at",
      "updated_at",
    ]) {
      expect(fields).toContain(required);
    }
  });
});
