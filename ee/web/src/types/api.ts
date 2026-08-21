export interface LoginResponse {
  api_key: string;
  api_key_id: string;
  workspace_id: string;
  scope: string;
  user_id: string;
}

export interface SignupResponse {
  user_id: string;
  api_key: string;
  workspace_id: string;
}

export interface OAuthStatus {
  enabled: boolean;
}

/**
 * `setup_required` is true only when the deployment has a finite tenant cap and holds no tenant
 * yet. A hosted deployment sets the cap to unlimited, so it always reads false there.
 */
export interface SetupStatus {
  setup_required: boolean;
}

export interface SetupResponse {
  user_id: string;
  email: string;
  tenant_id: string;
  workspace_id: string;
  api_key: string;
}

export interface TenantOverview {
  tenant_id: string;
  plan: string | null;
  max_workspaces: number | null;
  usage: TenantUsage;
  members: MemberRecord[];
}

export interface TenantUsage {
  tenant_id: string;
  workspace_count: number;
  member_count: number;
  entity_count: number;
}

export interface MemberRecord {
  user_id: string;
  email: string;
  display_name: string | null;
  role: "owner" | "admin" | "member" | "viewer";
}

/// Generated from the server's own OpenAPI document (`pnpm run gen:api-types`) rather than
/// hand-transcribed, so a field the server adds or drops here is a type error rather than a
/// silent mismatch. See `generated-api.ts`'s header for how to regenerate it.
export type Workspace = import("./generated-api").components["schemas"]["WorkspaceRecord"];

export interface WorkspaceDetail {
  id: string;
  tenant_id: string;
  schema_id: string;
  name: string;
  max_entities: number | null;
  created_at: string;
  entity_count: number;
  relation_count: number;
  schema_count: number;
}

export interface Schema {
  id: string;
  name: string;
  version: number;
  status: string;
  created_at: string;
}

export interface SchemaDetail {
  id: string;
  tenant_id: string;
  name: string;
  version: number;
  definition: SchemaDefinition;
  status: string;
  created_at: string;
}

export interface SchemaDefinition {
  name: string;
  description: string | null;
  entity_types: Record<string, EntityTypeDef>;
  relation_types: Record<string, RelationTypeDef>;
}

export interface EntityTypeDef {
  description: string | null;
  fields: Record<string, FieldDef>;
}

export interface FieldDef {
  type: string;
  required: boolean;
  description?: string;
  "x-embed"?: boolean;
  "x-ui"?: { widget?: string };
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  format?: string;
  enum?: string[];
  default?: unknown;
  items?: { type: string };
  uniqueItems?: boolean;
  properties?: Record<string, FieldDef>;
}

export interface RelationTypeDef {
  source: string;
  target: string;
  description: string | null;
}

export interface Entity {
  id: string;
  workspace_id: string;
  schema_id: string;
  schema_version: number;
  entity_type: string;
  data: Record<string, unknown>;
  created_at: string;
  updated_at: string;
  created_by: string | null;
  updated_by: string | null;
}

/**
 * One neighbor of the entity `GET /api/entities/{id}/context` was asked about. Mirrors the
 * server's `RecallRelation`: the neighbor arrives as a nested entity, not as flattened
 * `neighbor_id`/`neighbor_type` fields, and `direction` is `"out"`/`"in"` rather than spelled out.
 */
export interface Relation {
  relation_type: string;
  direction: "out" | "in";
  /** Shallow by default -- only `x-embed` fields are populated in `data`. */
  neighbor: Entity;
  /** 1 = direct neighbor, 2 = neighbor-of-neighbor, and so on. */
  hop_distance: number;
}

/** Mirrors the server's `RecallContext`. */
export interface EntityContext {
  entity: Entity;
  relations: Relation[];
  /** `true` when more neighbors existed at some hop than the request's limit allowed. */
  truncated: boolean;
}

/**
 * One hit from `GET /api/search`. Mirrors the server's `SearchHit` -- the entity is nested, and
 * the endpoint returns `SearchHit[]`, never a bare `Entity[]`.
 *
 * `distance` is a pgvector cosine *distance*, so smaller means more similar -- it is not a
 * similarity score and must not be presented as one. `null` when the entity carries no embedding
 * and only surfaced through the fuzzy text match.
 */
export interface SearchHit {
  entity: Entity;
  distance: number | null;
}

export interface Template {
  id: string;
  name: string;
  description: string;
}

/** Mirrors the hosted API's `MarketplaceListing`. */
export interface MarketplaceListing {
  template_id: string;
  name: string;
  description: string | null;
  tags: string[];
  author: string | null;
  tenant_id: string;
  /** `null` when only pre-releases have been published. */
  latest_stable_version: number | null;
  review_count: number;
  /** `null` when nobody has reviewed it. */
  average_rating: number | null;
}

export interface TemplateVersion {
  id: string;
  template_id: string;
  version: number;
  /** A full metaschema, the same shape `/api/schemas` returns -- the detail page renders its
   *  structure graph and type tables from this without a second request. */
  definition: SchemaDefinition;
  changelog: string | null;
  /** `draft` | `pre` | `stable`. Drafts are only ever returned for your own templates. */
  status: string;
  created_at: string;
}

export interface TemplateReview {
  id: string;
  template_id: string;
  tenant_id: string;
  rating: number;
  comment: string | null;
  created_at: string;
  updated_at: string;
}

export interface WhoAmIResponse {
  workspace_id: string;
  tenant_id: string;
  scope: string;
  user_id: string | null;
}

/** Mirrors `yorishiro_core::models::import::ImportResult`. */
export interface ImportResult {
  schemas: number;
  entities: number;
  relations: number;
  /** Non-empty only when the import was rolled back; the counts are then not committed. */
  errors: string[];
}

/// Which columns the Entities table shows for one entity type, in display order.
///
/// Absent from the list entirely means the workspace has never chosen, and the table derives
/// columns from the schema. A present entry with an empty `columns` is a choice: show none.
export interface ColumnPreference {
  entity_type: string;
  columns: string[];
}
