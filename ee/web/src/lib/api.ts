import type {
  Entity,
  EntityContext,
  ImportResult,
  LoginResponse,
  MarketplaceListing,
  OAuthStatus,
  Schema,
  SchemaDefinition,
  SchemaDetail,
  SearchHit,
  SetupResponse,
  SetupStatus,
  SignupResponse,
  Template,
  TemplateReview,
  TemplateVersion,
  TenantOverview,
  WhoAmIResponse,
  Workspace,
  WorkspaceDetail,
} from "@/types/api";
import { errorMessage } from "./errorMessage";

const SESSION_KEY = "yorishiro_session";

export function getApiKey(): string | null {
  try {
    const raw = sessionStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const session = JSON.parse(raw) as { apiKey?: string };
    return session.apiKey ?? null;
  } catch {
    return null;
  }
}

export function setSession(apiKey: string, email: string, workspaceId?: string): void {
  // Preserves any workspace already recorded, so this cannot silently drop it depending on the
  // order the caller happens to write the two.
  const existing = getSessionWorkspaceId();
  sessionStorage.setItem(
    SESSION_KEY,
    JSON.stringify({ apiKey, email, workspaceId: workspaceId ?? existing ?? undefined }),
  );
}

export function clearSession(): void {
  sessionStorage.removeItem(SESSION_KEY);
}

export function getSessionEmail(): string | null {
  try {
    const raw = sessionStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const session = JSON.parse(raw) as { email?: string };
    return session.email ?? null;
  } catch {
    return null;
  }
}

/// One `{field, problem}` pair the server error carries. On a 422 workspace-ambiguity response
/// from `/auth/login`, `field` is a workspace id and `problem` is its display name; the generic
/// `ValidationDetail` shape is reused rather than adding a dedicated one.
export interface ValidationDetail {
  field: string;
  problem: string;
}

export class ApiError extends Error {
  status: number;
  details: ValidationDetail[] | null;

  constructor(status: number, message: string, details: ValidationDetail[] | null = null) {
    super(message);
    this.status = status;
    this.details = details;
    this.name = "ApiError";
  }
}

/// Reads `error.details` out of a hosted-API error body, when it is the `{field, problem}[]`
/// shape. Anything else (missing, wrong shape, a community-API `{message}` body) yields `null`
/// so the caller falls back to its pre-picker behaviour rather than trusting a malformed value.
function errorDetails(body: string): ValidationDetail[] | null {
  try {
    const json = JSON.parse(body);
    const details = json.error?.details;
    if (
      Array.isArray(details) &&
      details.every((d) => d && typeof d.field === "string" && typeof d.problem === "string")
    ) {
      return details as ValidationDetail[];
    }
  } catch {
    // Not JSON -- no details to offer.
  }
  return null;
}

/// The workspace the user is currently looking at, taken from the URL.
///
/// Every content endpoint scopes to a workspace, and without this header the server resolves
/// the one recorded on the API key -- so opening a second workspace showed the *first* one's
/// entities, schema and search results under the second one's URL. Reading the path rather than
/// threading a parameter through every call keeps the two from disagreeing.
///
/// A workspace-scoped key ignores the header when it names that key's own workspace, and is
/// refused when it names another, so sending it is safe for both kinds of key.
function currentWorkspaceId(): string | null {
  const match = /^\/ws\/([^/]+)/.exec(globalThis.location?.pathname ?? "");
  return match ? match[1] : null;
}

/// The workspace recorded on the API key itself, learned from `/whoami` at login.
///
/// A workspace-scoped key -- the only kind `/auth/login` issues -- is *refused* when the header
/// names a different workspace, rather than quietly acting on its own. So the header is sent
/// only when the URL names some other workspace, which is exactly when the request would
/// otherwise be answered for the wrong one.
export function getSessionWorkspaceId(): string | null {
  try {
    const raw = sessionStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    return (JSON.parse(raw) as { workspaceId?: string }).workspaceId ?? null;
  } catch {
    return null;
  }
}

export function setSessionWorkspaceId(workspaceId: string): void {
  try {
    const raw = sessionStorage.getItem(SESSION_KEY);
    const session = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
    sessionStorage.setItem(SESSION_KEY, JSON.stringify({ ...session, workspaceId }));
  } catch {
    // A session we cannot parse is one the user will be asked to sign in for anyway.
  }
}

/// Turns a failed response into the message the user sees.
///
/// Shared by all three fetch paths in this file, which had each grown their own copy of the
/// status mapping.
async function failureMessage(res: Response): Promise<string> {
  return failureMessageFromText(res, await res.text());
}

/// Same mapping as `failureMessage`, taking an already-read body so a caller that also wants
/// `errorDetails(text)` does not consume the response stream twice.
function failureMessageFromText(res: Response, text: string): string {
  const message = errorMessage(text, res.statusText);
  if (res.status === 401) return "Session expired. Please sign in again.";
  // The workspace header is refused when the key is bound to a different workspace. Saying so
  // in the API's own words leaves the reader with a header name and no next step; this names
  // the one action that resolves it.
  if (/workspace this key cannot act on/i.test(message)) {
    return "This sign-in is limited to one workspace. Sign in again choosing this workspace to open it.";
  }
  if (res.status === 403) return "You don't have permission for this action.";
  return message;
}

async function request<T>(path: string, options: RequestInit = {}): Promise<T> {
  const apiKey = getApiKey();
  const headers: Record<string, string> = {
    ...(options.headers as Record<string, string>),
  };
  if (apiKey) {
    headers["Authorization"] = `Bearer ${apiKey}`;
  }
  const workspaceId = currentWorkspaceId();
  if (workspaceId && workspaceId !== getSessionWorkspaceId() && !headers["X-Workspace-Id"]) {
    headers["X-Workspace-Id"] = workspaceId;
  }
  if (options.body && typeof options.body === "string") {
    headers["Content-Type"] = "application/json";
  }
  const res = await fetch(path, { ...options, headers });
  if (!res.ok) {
    const text = await res.text();
    throw new ApiError(res.status, failureMessageFromText(res, text), errorDetails(text));
  }
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

// Auth
export async function login(
  email: string,
  password: string,
  workspaceId?: string,
): Promise<LoginResponse> {
  const body: Record<string, string> = { email, password };
  if (workspaceId) body.workspace_id = workspaceId;
  return request<LoginResponse>("/auth/login", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function signup(
  email: string,
  password: string,
  token: string,
  displayName?: string,
): Promise<SignupResponse> {
  const body: Record<string, string> = { email, password, invite_token: token };
  if (displayName) body.display_name = displayName;
  return request<SignupResponse>("/auth/signup", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function getOAuthStatus(): Promise<OAuthStatus> {
  return request<OAuthStatus>("/auth/oauth/status");
}

export async function getSetupStatus(): Promise<SetupStatus> {
  return request<SetupStatus>("/setup/status");
}

/**
 * Creates the deployment's first tenant, workspace and owner account.
 *
 * Answers 404 when the wizard is disabled (the tenant cap is unlimited, which is how a hosted
 * deployment runs) and 409 when a tenant already exists. Both are ordinary outcomes rather than
 * faults, so the caller distinguishes them by status.
 */
export async function setup(
  email: string,
  password: string,
  displayName?: string,
): Promise<SetupResponse> {
  const body: Record<string, string> = { email, password };
  if (displayName) body.display_name = displayName;
  return request<SetupResponse>("/setup", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function whoami(): Promise<WhoAmIResponse> {
  return request<WhoAmIResponse>("/whoami");
}

// Tenant/Dashboard
export async function getTenantOverview(): Promise<TenantOverview> {
  return request<TenantOverview>("/hosted/tenant/overview");
}

export async function listMarketplace(): Promise<MarketplaceListing[]> {
  return request<MarketplaceListing[]>("/api/marketplace");
}

export async function listTemplateVersions(templateId: string): Promise<TemplateVersion[]> {
  return request<TemplateVersion[]>(`/api/marketplace/${encodeURIComponent(templateId)}/versions`);
}

export async function listTemplateReviews(templateId: string): Promise<TemplateReview[]> {
  return request<TemplateReview[]>(`/api/marketplace/${encodeURIComponent(templateId)}/reviews`);
}

export async function submitTemplateReview(
  templateId: string,
  rating: number,
  comment: string | null,
): Promise<TemplateReview> {
  return request<TemplateReview>(`/api/marketplace/${encodeURIComponent(templateId)}/reviews`, {
    method: "POST",
    body: JSON.stringify({ rating, comment }),
  });
}

/** Omitting `version` forks the latest stable one. */
export async function forkMarketplaceTemplate(
  templateId: string,
  version?: number,
): Promise<{ template_id: string }> {
  const qs = version === undefined ? "" : `?version=${version}`;
  return request<{ template_id: string }>(
    `/api/marketplace/${encodeURIComponent(templateId)}/fork${qs}`,
    { method: "POST" },
  );
}

export async function addMember(email: string, role: string): Promise<void> {
  return request("/hosted/tenant/members", {
    method: "POST",
    body: JSON.stringify({ email, role }),
  });
}

// Workspaces
export async function listWorkspaces(): Promise<Workspace[]> {
  return request<Workspace[]>("/api/workspaces");
}

export async function getWorkspace(id: string): Promise<WorkspaceDetail> {
  return request<WorkspaceDetail>(`/api/workspaces/${id}`);
}

export async function createWorkspace(
  name: string,
  schemaId: string,
  maxEntities?: number,
): Promise<Workspace> {
  const body: Record<string, unknown> = { name, schema_id: schemaId };
  if (maxEntities !== undefined) body.max_entities = maxEntities;
  return request<Workspace>("/api/workspaces", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

// Schemas
export async function listSchemas(): Promise<Schema[]> {
  return request<Schema[]>("/api/schemas");
}

export async function getActiveSchema(name: string): Promise<SchemaDetail> {
  return request<SchemaDetail>(`/api/schemas/active/${encodeURIComponent(name)}`);
}

export async function getSchemaById(id: string): Promise<SchemaDetail> {
  return request<SchemaDetail>(`/api/schemas/${encodeURIComponent(id)}`);
}

export async function createSchema(
  definition: Record<string, unknown>,
): Promise<{ schema: SchemaDetail }> {
  return request("/api/schemas", {
    method: "POST",
    body: JSON.stringify(definition),
  });
}

export async function createSchemaFromTemplate(
  templateId: string,
): Promise<{ schema: SchemaDetail }> {
  return request("/api/schemas", {
    method: "POST",
    body: JSON.stringify({ template_id: templateId }),
  });
}

export async function listTemplates(): Promise<Template[]> {
  return request<Template[]>("/api/templates");
}

export async function getTemplate(id: string): Promise<SchemaDefinition> {
  return request<SchemaDefinition>(`/api/templates/${id}`);
}

// Entities
export async function listEntities(params?: {
  entity_type?: string;
  offset?: number;
  limit?: number;
}): Promise<Entity[]> {
  const searchParams = new URLSearchParams();
  if (params?.entity_type) searchParams.set("entity_type", params.entity_type);
  if (params?.offset) searchParams.set("offset", String(params.offset));
  if (params?.limit) searchParams.set("limit", String(params.limit));
  const qs = searchParams.toString();
  return request<Entity[]>(`/api/entities${qs ? `?${qs}` : ""}`);
}

export async function getEntity(id: string): Promise<Entity> {
  return request<Entity>(`/api/entities/${id}`);
}

export async function createEntity(
  schemaName: string,
  entityType: string,
  data: Record<string, unknown>,
): Promise<Entity> {
  return request<Entity>("/api/entities", {
    method: "POST",
    body: JSON.stringify({ schema_name: schemaName, entity_type: entityType, data }),
  });
}

export async function updateEntity(id: string, data: Record<string, unknown>): Promise<Entity> {
  return request<Entity>(`/api/entities/${id}`, {
    method: "PUT",
    body: JSON.stringify({ data }),
  });
}

export async function deleteEntity(id: string): Promise<void> {
  return request(`/api/entities/${id}`, { method: "DELETE" });
}

export async function getEntityContext(id: string, depth?: number): Promise<EntityContext> {
  const qs = depth ? `?depth=${depth}` : "";
  return request<EntityContext>(`/api/entities/${id}/context${qs}`);
}

// Search
export async function searchEntities(
  queryText: string,
  params?: { entity_type?: string; limit?: number },
): Promise<SearchHit[]> {
  const searchParams = new URLSearchParams({ query_text: queryText });
  if (params?.entity_type) searchParams.set("entity_type", params.entity_type);
  if (params?.limit) searchParams.set("limit", String(params.limit));
  return request<SearchHit[]>(`/api/search?${searchParams}`);
}

// Import / Export
export async function exportJsonl(): Promise<string> {
  const apiKey = getApiKey();
  const headers: Record<string, string> = {};
  if (apiKey) {
    headers["Authorization"] = `Bearer ${apiKey}`;
  }
  const res = await fetch("/api/export.jsonl", { headers });
  if (!res.ok) {
    throw new ApiError(res.status, await failureMessage(res));
  }
  return res.text();
}

export async function importJsonl(content: string): Promise<ImportResult> {
  const apiKey = getApiKey();
  const headers: Record<string, string> = {
    "Content-Type": "application/x-ndjson",
  };
  if (apiKey) {
    headers["Authorization"] = `Bearer ${apiKey}`;
  }
  const res = await fetch("/api/import.jsonl", {
    method: "POST",
    headers,
    body: content,
  });
  if (!res.ok) {
    throw new ApiError(res.status, await failureMessage(res));
  }
  return res.json() as Promise<ImportResult>;
}
