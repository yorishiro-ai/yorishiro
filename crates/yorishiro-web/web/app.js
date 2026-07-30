// Yorishiro admin dashboard -- a deliberately framework-free SPA. Scope is limited to first-run
// setup, login, usage/billing display, member management, workspace management (create/
// list/delete plus a summary detail view -- entity/relation/schema counts, not their content),
// and basic read/browse/edit access to schemas and entities (list + detail views, single-field
// data edits). It is not a full entity/schema/relation editor (no create/delete of schemas or
// entities, no relation management) -- that's what the REST API + Swagger UI (`/docs` on
// yorishiro-server) are for. Served by both yorishiro-server (via `YSR_WEB_DIR`) and
// yorishiro-hosted-server -- `/hosted/tenant/overview` and friends only exist on the latter, so
// on yorishiro-server alone, `#/dashboard` degrades to the API key login just issued plus
// workspace management (see `renderLoginComplete`) instead of the full hosted dashboard.

const SESSION_KEY = "yorishiro_session";

function apiBase() {
  return (window.YORISHIRO_CONFIG && window.YORISHIRO_CONFIG.apiBase) || "";
}

function getSession() {
  const raw = sessionStorage.getItem(SESSION_KEY);
  return raw ? JSON.parse(raw) : null;
}

function setSession(session) {
  sessionStorage.setItem(SESSION_KEY, JSON.stringify(session));
}

function clearSession() {
  sessionStorage.removeItem(SESSION_KEY);
}

function esc(str) {
  const d = document.createElement("div");
  d.textContent = str;
  return d.innerHTML;
}

function el(html) {
  const template = document.createElement("template");
  template.innerHTML = html.trim();
  return template.content.firstElementChild;
}

function mount(node) {
  const app = document.getElementById("app");
  app.replaceChildren(node);
}

async function parseErrorMessage(response) {
  try {
    const body = await response.json();
    return body?.error?.message || `request failed (${response.status})`;
  } catch {
    return `request failed (${response.status})`;
  }
}

// Like parseErrorMessage, but also surfaces `error.details` (field-level validation
// errors) and `error.hint` when present -- used where we want to show the server's full
// validation error rather than just its top-level message (see `ApiErrorBody` /
// `ApiErrorDetail` in yorishiro-server's error.rs).
async function parseErrorDetail(response) {
  try {
    const body = await response.json();
    const message = body?.error?.message || `request failed (${response.status})`;
    const parts = [message];
    if (Array.isArray(body?.error?.details) && body.error.details.length > 0) {
      parts.push(body.error.details.map((d) => `- ${JSON.stringify(d)}`).join("\n"));
    }
    if (body?.error?.hint) {
      parts.push(`Hint: ${body.error.hint}`);
    }
    return parts.join("\n");
  } catch {
    return `request failed (${response.status})`;
  }
}

async function checkSetupStatus() {
  try {
    const response = await fetch(`${apiBase()}/setup/status`);
    if (!response.ok) {
      return { setup_required: false };
    }
    return response.json();
  } catch {
    return { setup_required: false };
  }
}

async function setup({ email, password, displayName }) {
  const response = await fetch(`${apiBase()}/setup`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email, password, display_name: displayName || undefined }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function login({ email, password, workspaceId }) {
  const response = await fetch(`${apiBase()}/auth/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    // workspace_id is omitted (rather than sent as an empty string) when the field isn't
    // shown, so the server can auto-resolve it for the common single-workspace account.
    body: JSON.stringify({ email, password, workspace_id: workspaceId || undefined }),
  });
  if (!response.ok) {
    const error = new Error(await parseErrorMessage(response));
    error.status = response.status;
    throw error;
  }
  return response.json();
}

async function fetchTenantOverview(apiKey) {
  const response = await fetch("/hosted/tenant/overview", {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    const error = new Error(await parseErrorMessage(response));
    error.status = response.status;
    throw error;
  }
  return response.json();
}

async function addMember(apiKey, { email, role }) {
  const response = await fetch(`${apiBase()}/api/members`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ email, role }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function listWorkspaces(apiKey) {
  const response = await fetch(`${apiBase()}/api/workspaces`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function listTemplates(apiKey) {
  const response = await fetch(`${apiBase()}/api/templates`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) return [];
  return response.json();
}

async function createSchemaFromTemplate(apiKey, templateId) {
  const response = await fetch(`${apiBase()}/api/schemas`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ template_id: templateId }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

// Registers a schema from an inline MetaSchemaDefinition JSON body (as opposed to
// createSchemaFromTemplate's `{ template_id }` shorthand). Used by the AI schema generator's
// "Apply" step -- the server only validates and registers the definition; it never sees how
// the definition was produced.
async function createSchema(apiKey, definition) {
  const response = await fetch(`${apiBase()}/api/schemas`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify(definition),
  });
  if (!response.ok) {
    const error = new Error(await parseErrorDetail(response));
    error.status = response.status;
    throw error;
  }
  return response.json();
}

async function createWorkspace(apiKey, { name, maxEntities }) {
  const response = await fetch(`${apiBase()}/api/workspaces`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    // max_entities is omitted (rather than sent as an empty string) when left blank, so the
    // server applies its own "unlimited" default instead of failing to parse "" as a number.
    body: JSON.stringify({ name, max_entities: maxEntities || undefined }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function getWorkspace(apiKey, id) {
  const response = await fetch(`${apiBase()}/api/workspaces/${id}`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function deleteWorkspace(apiKey, id) {
  const response = await fetch(`${apiBase()}/api/workspaces/${id}`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
}

async function listTemplateLibrary(apiKey) {
  const response = await fetch(`${apiBase()}/api/template-library`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function createTemplateLibraryItem(apiKey, { name, description, definition, tags }) {
  const response = await fetch(`${apiBase()}/api/template-library`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({
      name,
      description: description || undefined,
      definition,
      tags: tags.length > 0 ? tags : undefined,
    }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function deleteTemplateLibraryItem(apiKey, id) {
  const response = await fetch(`${apiBase()}/api/template-library/${id}`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
}

// -- Entity/schema browsing (community edition: read/browse plus simple edit; no
// create/delete UI here -- use the REST API/MCP for that, see /docs). --

async function listSchemas(apiKey) {
  const response = await fetch(`${apiBase()}/api/schemas`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function getSchemaById(apiKey, schemaId) {
  const response = await fetch(`${apiBase()}/api/schemas/${schemaId}`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function getActiveSchema(apiKey, name) {
  const response = await fetch(`${apiBase()}/api/schemas/active/${encodeURIComponent(name)}`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function listEntities(apiKey, { entityType, limit = 50, offset = 0 } = {}) {
  const params = new URLSearchParams();
  if (entityType) params.set("entity_type", entityType);
  params.set("limit", String(limit));
  params.set("offset", String(offset));
  const response = await fetch(`${apiBase()}/api/entities?${params.toString()}`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function getEntity(apiKey, id) {
  const response = await fetch(`${apiBase()}/api/entities/${id}`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function getEntityContext(apiKey, id) {
  const response = await fetch(`${apiBase()}/api/entities/${id}/context`, {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function updateEntity(apiKey, id, data) {
  const response = await fetch(`${apiBase()}/api/entities/${id}`, {
    method: "PUT",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ data }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

// -- AI Schema Generator (client-side, BYO API key) --
//
// The LLM call happens entirely in the browser: the user's API key is sent directly to the
// LLM endpoint they specify (via fetch from this page), never to the Yorishiro server, and is
// never persisted anywhere (not even sessionStorage) -- it lives only in the form field and an
// in-memory closure for the duration of one click. The server's only role is the existing
// POST /api/schemas, which just validates and registers whatever MetaSchemaDefinition JSON the
// LLM produced.

const AI_SCHEMA_EXAMPLE = {
  name: "worldbuilding",
  description: "Characters, locations, factions, and items for creative writing and TRPG worldbuilding",
  entity_types: {
    character: {
      description: "A person or creature in the world",
      fields: {
        name: {
          type: "string",
          required: true,
          description: "Full name or alias",
          "x-embed": true,
          maxLength: 100,
        },
        description: {
          type: "string",
          description: "Appearance, personality, background",
          "x-embed": true,
        },
        traits: {
          type: "array",
          items: { type: "string" },
          description: "Personality traits or notable features",
        },
        status: {
          type: "string",
          enum: ["alive", "deceased", "unknown"],
          default: "alive",
        },
        profile: {
          type: "object",
          description: "Structured profile data",
          properties: {
            age: { type: "integer", minimum: 0 },
            occupation: { type: "string" },
          },
        },
      },
    },
    location: {
      description: "A place in the world",
      fields: {
        name: { type: "string", required: true, "x-embed": true, maxLength: 100 },
        description: { type: "string", "x-embed": true },
      },
    },
  },
  relation_types: {
    located_in: {
      source: "character",
      target: "location",
      description: "Character is currently at this location",
    },
  },
};

function buildSchemaGenerationPrompt(userDescription) {
  return `You are generating a Yorishiro schema definition. Yorishiro is a knowledge-graph store where data is organized into "entity types" (nodes, with typed fields) and "relation_types" (typed edges between entity types).

Output STRICT JSON matching this exact shape (a "MetaSchemaDefinition"), and nothing else -- no markdown fences, no commentary:

{
  "name": "string, a short machine-friendly identifier for this schema",
  "description": "string, optional, human-readable summary",
  "entity_types": {
    "<entity_type_name>": {
      "description": "string, optional",
      "fields": {
        "<field_name>": {
          "type": "string" | "number" | "integer" | "boolean" | "array" | "object",
          "required": true | false,
          "description": "string, optional",
          "x-embed": true | false,
          "enum": ["optional", "list", "of", "allowed", "string", "values"],
          "default": "optional default value",
          "items": { "type": "..." },
          "properties": { "...": { "type": "..." } }
        }
      }
    }
  },
  "relation_types": {
    "<relation_type_name>": {
      "source": "<entity_type_name>",
      "target": "<entity_type_name>",
      "description": "string, optional"
    }
  }
}

Field type rules:
- "type" must be one of: string, number, integer, boolean, array, object.
- For "array" fields, include "items" describing the element type (e.g. {"type": "string"}).
- For "object" fields, include "properties" describing nested fields (same field shape, can nest).
- Set "x-embed": true on the 1-3 fields per entity type that best represent it in free text (e.g. a name or description field) -- these are used for semantic search/embeddings. Do not set it on every field.
- Use "enum" for fields with a small fixed set of allowed string values.
- Mark fields "required": true only when the data genuinely cannot exist without them.
- "relation_types" describes directed edges between entity types; "source" and "target" must reference keys in "entity_types".

Example of a valid, complete schema definition:
${JSON.stringify(AI_SCHEMA_EXAMPLE, null, 2)}

Now generate a schema definition for the following data structure, described by the user in natural language:

"""
${userDescription}
"""

Respond with ONLY the JSON object, no surrounding text.`;
}

// Auto-detects OpenAI-compatible vs. Anthropic-compatible request/response shape from the
// endpoint URL. Anthropic endpoints get POST {endpoint}/messages with x-api-key + a
// max_tokens field; everything else is treated as OpenAI-compatible: POST
// {endpoint}/chat/completions with an Authorization: Bearer header.
function isAnthropicEndpoint(endpoint) {
  return endpoint.toLowerCase().includes("anthropic");
}

async function callLlm({ endpoint, apiKey, model, prompt }) {
  const base = endpoint.replace(/\/+$/, "");
  const anthropic = isAnthropicEndpoint(base);

  const url = anthropic ? `${base}/messages` : `${base}/chat/completions`;
  const headers = { "content-type": "application/json" };
  let body;

  if (anthropic) {
    headers["x-api-key"] = apiKey;
    headers["anthropic-version"] = "2023-06-01";
    // Anthropic's browser SDK/API normally requires this header to allow direct
    // browser calls; harmless to send against any Anthropic-compatible proxy too.
    headers["anthropic-dangerous-direct-browser-access"] = "true";
    body = {
      model,
      max_tokens: 4096,
      messages: [{ role: "user", content: prompt }],
    };
  } else {
    headers.authorization = `Bearer ${apiKey}`;
    body = {
      model,
      messages: [{ role: "user", content: prompt }],
    };
  }

  let response;
  try {
    response = await fetch(url, { method: "POST", headers, body: JSON.stringify(body) });
  } catch (err) {
    throw new Error(`could not reach LLM endpoint "${url}": ${err.message}`);
  }

  if (!response.ok) {
    let detail = `request failed (${response.status})`;
    try {
      const errBody = await response.json();
      detail = errBody?.error?.message || errBody?.message || JSON.stringify(errBody);
    } catch {
      // ignore, use status-only detail
    }
    throw new Error(`LLM endpoint returned an error: ${detail}`);
  }

  const json = await response.json();

  // Extract the assistant's text from either response shape.
  let text;
  if (anthropic) {
    text = (json.content || [])
      .filter((block) => block.type === "text")
      .map((block) => block.text)
      .join("");
  } else {
    text = json.choices?.[0]?.message?.content;
  }

  if (!text) {
    throw new Error(
      `LLM response did not contain any text content. Raw response: ${JSON.stringify(json)}`,
    );
  }

  return text;
}

// The LLM is asked to reply with only JSON, but real-world responses sometimes wrap it in
// markdown fences or add a stray sentence -- so we try a strict parse first, then fall back to
// locating the outermost {...} block.
function extractJsonFromLlmResponse(text) {
  try {
    return JSON.parse(text);
  } catch {
    // fall through to extraction below
  }

  const fenceMatch = text.match(/```(?:json)?\s*([\s\S]*?)```/i);
  if (fenceMatch) {
    try {
      return JSON.parse(fenceMatch[1].trim());
    } catch {
      // fall through
    }
  }

  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start !== -1 && end !== -1 && end > start) {
    try {
      return JSON.parse(text.slice(start, end + 1));
    } catch {
      // fall through
    }
  }

  const err = new Error("LLM response did not contain valid JSON.");
  err.rawResponse = text;
  throw err;
}

async function generateSchemaFromDescription({ description, endpoint, apiKey, model }) {
  const prompt = buildSchemaGenerationPrompt(description);
  const text = await callLlm({ endpoint, apiKey, model, prompt });
  return extractJsonFromLlmResponse(text);
}

function renderAiSchemaGenerator(state = {}) {
  const { error, rawResponse, generatedSchema, applyError, applyResult } = state;

  const view = el(`
    <div class="ai-schema-generator">
      <h2>AI Schema Generator</h2>
      <p class="hint">Describe your data structure in plain language and an LLM will draft a
      schema definition for you to review before applying it.</p>
      <p class="warning">Your API key is sent directly to the LLM endpoint you specify below.
      It is never stored (not even in this browser) and never sent to this server.</p>

      <form id="ai-schema-form">
        <label>Describe your data structure
          <textarea name="description" rows="4" required placeholder="e.g. I want to track characters, locations, and their relationships for my novel">${esc(state.description || "")}</textarea>
        </label>
        <label>LLM API endpoint
          <input type="text" name="endpoint" required placeholder="https://api.anthropic.com/v1 or https://api.openai.com/v1" value="${esc(state.endpoint || "")}">
        </label>
        <label>API key
          <input type="password" name="apiKey" required autocomplete="off" placeholder="sk-...">
        </label>
        <label>Model
          <input type="text" name="model" value="${esc(state.model || "claude-sonnet-4-20250514")}" placeholder="claude-sonnet-4-20250514 or gpt-4o">
        </label>
        <button type="submit" id="ai-generate-button">Generate</button>
      </form>

      ${error ? `<p class="error">${esc(error)}</p>` : ""}
      ${
        rawResponse
          ? `<details open><summary>Raw LLM response</summary><pre>${esc(rawResponse)}</pre></details>`
          : ""
      }

      ${
        generatedSchema
          ? `
            <h3>Generated schema preview</h3>
            <pre id="ai-schema-preview">${esc(JSON.stringify(generatedSchema, null, 2))}</pre>
            ${applyError ? `<p class="error">${esc(applyError)}</p>` : ""}
            ${applyResult ? `<p class="success">Schema "${esc(applyResult.schema.name)}" registered (version ${esc(String(applyResult.schema.version))}).</p>` : ""}
            <button type="button" id="ai-apply-button">Apply</button>
          `
          : ""
      }
    </div>
  `);

  view.querySelector("#ai-schema-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    const description = form.get("description");
    const endpoint = form.get("endpoint");
    const apiKey = form.get("apiKey");
    const model = form.get("model") || "claude-sonnet-4-20250514";

    const generateButton = view.querySelector("#ai-generate-button");
    generateButton.disabled = true;
    generateButton.textContent = "Generating…";

    try {
      const generatedSchema = await generateSchemaFromDescription({
        description,
        endpoint,
        apiKey,
        model,
      });
      const rendered = renderAiSchemaGenerator({ description, endpoint, model, generatedSchema });
      view.replaceWith(rendered);
    } catch (err) {
      const rendered = renderAiSchemaGenerator({
        description,
        endpoint,
        model,
        error: err.message,
        rawResponse: err.rawResponse,
      });
      view.replaceWith(rendered);
    } finally {
      // `view` may have been replaced already; guard in case the button element is stale.
      generateButton.disabled = false;
      generateButton.textContent = "Generate";
    }
  });

  const applyButton = view.querySelector("#ai-apply-button");
  if (applyButton) {
    applyButton.addEventListener("click", async () => {
      const session = getSession();
      if (!session) {
        location.hash = "#/login";
        return;
      }
      applyButton.disabled = true;
      applyButton.textContent = "Applying…";
      try {
        const applyResult = await createSchema(session.apiKey, generatedSchema);
        const rendered = renderAiSchemaGenerator({
          description: state.description,
          endpoint: state.endpoint,
          model: state.model,
          generatedSchema,
          applyResult,
        });
        view.replaceWith(rendered);
      } catch (err) {
        const rendered = renderAiSchemaGenerator({
          description: state.description,
          endpoint: state.endpoint,
          model: state.model,
          generatedSchema,
          applyError: err.message,
        });
        view.replaceWith(rendered);
      }
    });
  }

  return view;
}

async function renderSetup(errorMessage) {

  const view = el(`
    <div>
      <h1>Welcome to Yorishiro</h1>
      <p class="hint">This deployment has no tenant yet. Create the owner account to get started.</p>
      <form id="setup-form">
        <label>Email
          <input type="email" name="email" required autocomplete="username">
        </label>
        <label>Password
          <input type="password" name="password" required autocomplete="new-password" minlength="8">
        </label>
        <label>Display name (optional)
          <input type="text" name="displayName" autocomplete="name">
        </label>
        <label>Schema template
          <select name="templateId">
            <option value="">None — start with an empty workspace</option>
            <option value="general-notes">general-notes — A general-purpose note-taking schema with tags and links</option>
            <option value="task-management">task-management — Personal tasks and projects with deadlines and dependencies</option>
            <option value="worldbuilding">worldbuilding — Characters, locations, factions, and items for creative writing and TRPG</option>
            <option value="software-adr">software-adr — Architecture Decision Records, service catalog, and incident post-mortems</option>
          </select>
        </label>
        ${errorMessage ? `<p class="error">${errorMessage}</p>` : ""}
        <button type="submit">Create owner account</button>
      </form>
    </div>
  `);

  view.querySelector("#setup-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    const templateId = form.get("templateId");
    try {
      const result = await setup({
        email: form.get("email"),
        password: form.get("password"),
        displayName: form.get("displayName"),
      });
      if (templateId) {
        try {
          await createSchemaFromTemplate(result.api_key, templateId);
        } catch {
          // Schema creation is best-effort during setup; the workspace
          // is already usable and the template can be applied later via
          // the API or MCP.
        }
      }
      mount(renderSetupComplete(result));
    } catch (err) {
      mount(await renderSetup(err.message));
    }
  });

  return view;
}

function renderSetupComplete(result) {
  return el(`
    <div>
      <h1>Setup complete</h1>
      <p class="hint">The tenant, workspace, and owner account have been created.</p>
      <dl>
        <dt>Email</dt><dd>${esc(result.email)}</dd>
        <dt>Workspace ID</dt><dd><code>${esc(result.workspace_id)}</code></dd>
      </dl>
      <p class="error"><strong>Save this API key now -- it is only ever shown once:</strong></p>
      <pre>${result.api_key}</pre>
      <p class="hint">Use it as a Bearer token against the REST API (see <a href="/docs">/docs</a>) or your MCP client's configuration.</p>
      <p><a href="#/login">Continue to sign in</a></p>
    </div>
  `);
}

// `workspace_id` is only asked for when the account has access to more than one workspace --
// the server reports that with a 422, which is when `needsWorkspaceId` flips to true. Every
// community-edition deployment has exactly one workspace by default, so the common case never
// shows this field at all.
function renderLogin(errorMessage, needsWorkspaceId = false) {
  const view = el(`
    <div>
      <h1>Yorishiro</h1>
      <p class="hint">Sign in with the account created via setup or an invite (see /auth/signup).</p>
      <form id="login-form">
        <label>Email
          <input type="email" name="email" required autocomplete="username">
        </label>
        <label>Password
          <input type="password" name="password" required autocomplete="current-password">
        </label>
        ${
          needsWorkspaceId
            ? `<label>Workspace ID
                 <input type="text" name="workspaceId" required placeholder="00000000-0000-0000-0000-000000000000">
               </label>
               <p class="hint">This account has access to more than one workspace -- find its id in your signup response, or ask a tenant owner.</p>`
            : ""
        }
        ${errorMessage ? `<p class="error">${errorMessage}</p>` : ""}
        <button type="submit">Sign in</button>
      </form>
    </div>
  `);

  view.querySelector("#login-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    try {
      const result = await login({
        email: form.get("email"),
        password: form.get("password"),
        workspaceId: form.get("workspaceId"),
      });
      setSession({ apiKey: result.api_key, email: result.email ?? form.get("email") });
      location.hash = "#/dashboard";
    } catch (err) {
      mount(renderLogin(err.message, needsWorkspaceId || err.status === 422));
    }
  });

  return view;
}

function renderWorkspacesTable(workspaces) {
  if (workspaces.length === 0) {
    return `<p class="hint">No workspaces yet.</p>`;
  }
  const rows = workspaces
    .map(
      (ws) => `
        <tr>
          <td><a href="#/workspaces/${ws.id}">${esc(ws.name)}</a></td>
          <td>${ws.max_entities ?? "unlimited"}</td>
          <td>${new Date(ws.created_at).toLocaleString()}</td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>Name</th><th>Max entities</th><th>Created</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

function renderTemplateLibraryTable(templates) {
  if (templates.length === 0) {
    return `<p class="hint">No templates yet.</p>`;
  }
  const rows = templates
    .map(
      (tpl) => `
        <tr>
          <td>${esc(tpl.name)}</td>
          <td>${esc(tpl.description ?? "")}</td>
          <td>${tpl.tags.join(", ")}</td>
          <td><button class="danger" data-delete-template="${tpl.id}">Delete</button></td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>Name</th><th>Description</th><th>Tags</th><th></th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

// The community edition has no `/hosted/tenant/overview` dashboard, so this is what
// `renderDashboard` falls back to: the just-issued API key, plus workspace management
// (create/list/select -- see `renderWorkspaceDetail` for delete) since a self-hosted deployment
// otherwise has no way to see or manage workspaces beyond the one `/setup` created. Also hosts
// the tenant's DB-backed template library (distinct from the built-in `/api/templates` offered
// during setup) since there is no other admin surface for it in the community edition.
async function renderLoginComplete(session, createError, templateError) {
  let workspaces;
  try {
    workspaces = await listWorkspaces(session.apiKey);
  } catch (err) {
    workspaces = [];
  }

  let templates;
  try {
    templates = await listTemplateLibrary(session.apiKey);
  } catch (err) {
    templates = [];
  }

  const view = el(`
    <div>
      <div class="top-bar">
        <h1>Signed in</h1>
        <button class="secondary" id="logout-button">Sign out</button>
      </div>
      <p class="hint">Use the API key below as a Bearer token against the REST API (see
      <a href="/docs">/docs</a>) or your MCP client's configuration.</p>
      <dl>
        <dt>Email</dt><dd>${esc(session.email)}</dd>
      </dl>
      <pre>${session.apiKey}</pre>

      <p>
        <a href="#/schemas">Browse schemas</a>
        &middot;
        <a href="#/entities">Browse entities</a>
      </p>

      <div id="ai-schema-generator-slot"></div>

      <h2>Workspaces</h2>
      ${renderWorkspacesTable(workspaces)}

      <h2>Create a workspace</h2>
      <form id="create-workspace-form">
        <label>Name
          <input type="text" name="name" required>
        </label>
        <label>Max entities (optional)
          <input type="number" name="maxEntities" min="1">
        </label>
        ${createError ? `<p class="error">${createError}</p>` : ""}
        <button type="submit">Create workspace</button>
      </form>

      <h2>Template Library</h2>
      <p class="hint">Templates your tenant has saved for reuse when creating schemas (via the
      REST API's <code>template_id</code> or MCP's <code>create_schema</code>). Distinct from the
      built-in templates offered during setup.</p>
      <div id="template-library-table">${renderTemplateLibraryTable(templates)}</div>

      <h3>Add a template</h3>
      <p class="hint">Paste a schema definition JSON (the same shape used by
      <a href="/docs">POST /api/schemas</a>).</p>
      <form id="create-template-form">
        <label>Name
          <input type="text" name="name" required>
        </label>
        <label>Description (optional)
          <input type="text" name="description">
        </label>
        <label>Tags (comma-separated, optional)
          <input type="text" name="tags" placeholder="notes, personal">
        </label>
        <label>Definition (JSON)
          <textarea name="definition" rows="8" required placeholder='{"name": "...", "entity_types": {...}}'></textarea>
        </label>
        ${templateError ? `<p class="error">${templateError}</p>` : ""}
        <button type="submit">Add template</button>
      </form>
    </div>
  `);

  view.querySelector("#logout-button").addEventListener("click", () => {
    clearSession();
    location.hash = "#/login";
  });

  view.querySelector("#ai-schema-generator-slot").replaceWith(renderAiSchemaGenerator());

  view.querySelector("#create-workspace-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    try {
      await createWorkspace(session.apiKey, {
        name: form.get("name"),
        maxEntities: form.get("maxEntities"),
      });
      mount(await renderLoginComplete(session));
    } catch (err) {
      mount(await renderLoginComplete(session, err.message));
    }
  });

  view.querySelector("#create-template-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    const tags = (form.get("tags") || "")
      .split(",")
      .map((tag) => tag.trim())
      .filter((tag) => tag.length > 0);
    let definition;
    try {
      definition = JSON.parse(form.get("definition"));
    } catch {
      mount(await renderLoginComplete(session, undefined, "definition must be valid JSON"));
      return;
    }
    try {
      await createTemplateLibraryItem(session.apiKey, {
        name: form.get("name"),
        description: form.get("description"),
        definition,
        tags,
      });
      mount(await renderLoginComplete(session));
    } catch (err) {
      mount(await renderLoginComplete(session, undefined, err.message));
    }
  });

  view.querySelectorAll("[data-delete-template]").forEach((button) => {
    button.addEventListener("click", async () => {
      const id = button.getAttribute("data-delete-template");
      const confirmed = confirm("Delete this template? This cannot be undone.");
      if (!confirmed) {
        return;
      }
      try {
        await deleteTemplateLibraryItem(session.apiKey, id);
        mount(await renderLoginComplete(session));
      } catch (err) {
        mount(await renderLoginComplete(session, undefined, err.message));
      }
    });
  });

  return view;
}

// -- Schema browsing --

function renderSchemasTable(schemas) {
  if (schemas.length === 0) {
    return `<p class="hint">No schemas yet. Create one via the REST API or MCP (see <a href="/docs">/docs</a>).</p>`;
  }
  const rows = schemas
    .map(
      (schema) => `
        <tr>
          <td><a href="#/schemas/${encodeURIComponent(schema.name)}">${esc(schema.name)}</a></td>
          <td>${esc(String(schema.version))}</td>
          <td>${esc(schema.status)}</td>
          <td>${new Date(schema.created_at).toLocaleString()}</td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>Name</th><th>Version</th><th>Status</th><th>Created</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

async function renderSchemasRoute() {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const schemas = await listSchemas(session.apiKey);
    const view = el(`
      <div>
        <div class="top-bar">
          <h1>Schemas</h1>
          <button class="secondary" id="logout-button">Sign out</button>
        </div>
        <p><a href="#/dashboard">&larr; Back</a></p>
        ${renderSchemasTable(schemas)}
      </div>
    `);
    view.querySelector("#logout-button").addEventListener("click", () => {
      clearSession();
      location.hash = "#/login";
    });
    mount(view);
  } catch (err) {
    mount(el(`<p class="error">${esc(err.message)}</p><p><a href="#/dashboard">&larr; Back</a></p>`));
  }
}

function renderEntityTypesList(entityTypes) {
  const names = Object.keys(entityTypes).sort();
  if (names.length === 0) {
    return `<p class="hint">No entity types defined.</p>`;
  }
  return names
    .map((name) => {
      const def = entityTypes[name];
      const fields = Object.keys(def.fields || {}).sort();
      const fieldItems = fields
        .map((fieldName) => {
          const field = def.fields[fieldName];
          return `<li><code>${esc(fieldName)}</code>: ${esc(field.type)}${field.required ? " (required)" : ""}${field.description ? ` &mdash; ${esc(field.description)}` : ""}</li>`;
        })
        .join("");
      return `
        <div class="stat">
          <strong>${esc(name)}</strong>
          ${def.description ? `<p class="hint">${esc(def.description)}</p>` : ""}
          <ul>${fieldItems || `<li class="hint">no fields</li>`}</ul>
        </div>
      `;
    })
    .join("");
}

async function renderSchemaDetailRoute(name) {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const schema = await getActiveSchema(session.apiKey, name);
    const view = el(`
      <div>
        <div class="top-bar">
          <h1>${esc(schema.name)}</h1>
          <button class="secondary" id="logout-button">Sign out</button>
        </div>
        <p><a href="#/schemas">&larr; Back to schemas</a></p>

        <dl>
          <dt>Version</dt><dd>${esc(String(schema.version))}</dd>
          <dt>Status</dt><dd>${esc(schema.status)}</dd>
          <dt>Created</dt><dd>${new Date(schema.created_at).toLocaleString()}</dd>
        </dl>

        <h2>Entity types</h2>
        <div class="stat-grid">${renderEntityTypesList(schema.definition.entity_types)}</div>

        <h2>Full definition</h2>
        <pre>${esc(JSON.stringify(schema.definition, null, 2))}</pre>
      </div>
    `);
    view.querySelector("#logout-button").addEventListener("click", () => {
      clearSession();
      location.hash = "#/login";
    });
    mount(view);
  } catch (err) {
    mount(
      el(`<p class="error">${esc(err.message)}</p><p><a href="#/schemas">&larr; Back to schemas</a></p>`),
    );
  }
}

// -- Entity browsing --

function truncate(str, max) {
  return str.length > max ? `${str.slice(0, max)}…` : str;
}

function renderEntitiesTable(entities) {
  if (entities.length === 0) {
    return `<p class="hint">No entities found.</p>`;
  }
  const rows = entities
    .map(
      (entity) => `
        <tr>
          <td><a href="#/entities/${entity.id}"><code>${esc(entity.id.slice(0, 8))}&hellip;</code></a></td>
          <td>${esc(entity.entity_type)}</td>
          <td>${esc(truncate(JSON.stringify(entity.data), 100))}</td>
          <td>${new Date(entity.created_at).toLocaleString()}</td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>ID</th><th>Type</th><th>Data preview</th><th>Created</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

const ENTITIES_PAGE_SIZE = 50;

async function renderEntitiesRoute(entityType, offset = 0) {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const [entities, schemas] = await Promise.all([
      listEntities(session.apiKey, { entityType, limit: ENTITIES_PAGE_SIZE, offset }),
      listSchemas(session.apiKey).catch(() => []),
    ]);

    // Entity type options are gathered from every active schema's entity_types, since
    // entities aren't scoped to a single schema in this list view.
    const activeSchemaNames = [...new Set(schemas.filter((s) => s.status === "active").map((s) => s.name))];
    const activeSchemas = await Promise.all(
      activeSchemaNames.map((n) => getActiveSchema(session.apiKey, n).catch(() => null)),
    );
    const entityTypeOptions = [
      ...new Set(
        activeSchemas
          .filter(Boolean)
          .flatMap((schema) => Object.keys(schema.definition.entity_types)),
      ),
    ].sort();

    const view = el(`
      <div>
        <div class="top-bar">
          <h1>Entities</h1>
          <button class="secondary" id="logout-button">Sign out</button>
        </div>
        <p><a href="#/dashboard">&larr; Back</a></p>

        <label>Filter by entity type
          <select id="entity-type-filter">
            <option value="">All types</option>
            ${entityTypeOptions
              .map(
                (t) =>
                  `<option value="${esc(t)}" ${t === entityType ? "selected" : ""}>${esc(t)}</option>`,
              )
              .join("")}
          </select>
        </label>

        ${renderEntitiesTable(entities)}

        <p>
          <button id="load-more-button" ${entities.length < ENTITIES_PAGE_SIZE ? "disabled" : ""}>Load more</button>
        </p>
      </div>
    `);

    view.querySelector("#logout-button").addEventListener("click", () => {
      clearSession();
      location.hash = "#/login";
    });

    view.querySelector("#entity-type-filter").addEventListener("change", (event) => {
      renderEntitiesRoute(event.target.value || undefined, 0);
    });

    view.querySelector("#load-more-button").addEventListener("click", () => {
      renderEntitiesRoute(entityType, offset + ENTITIES_PAGE_SIZE);
    });

    mount(view);
  } catch (err) {
    mount(el(`<p class="error">${esc(err.message)}</p><p><a href="#/dashboard">&larr; Back</a></p>`));
  }
}

function renderRelationsList(relations) {
  if (relations.length === 0) {
    return `<p class="hint">No relations found.</p>`;
  }
  return `
    <table>
      <thead><tr><th>Relation</th><th>Direction</th><th>Neighbor</th><th>Hops</th></tr></thead>
      <tbody>
        ${relations
          .map(
            (rel) => `
              <tr>
                <td>${esc(rel.relation_type)}</td>
                <td>${esc(rel.direction)}</td>
                <td><a href="#/entities/${rel.neighbor.id}"><code>${esc(rel.neighbor.id.slice(0, 8))}&hellip;</code></a> (${esc(rel.neighbor.entity_type)})</td>
                <td>${esc(String(rel.hop_distance))}</td>
              </tr>`,
          )
          .join("")}
      </tbody>
    </table>
  `;
}

async function renderEntityDetailRoute(id, { editing = false, editError } = {}) {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const [entity, context] = await Promise.all([
      getEntity(session.apiKey, id),
      getEntityContext(session.apiKey, id).catch(() => null),
    ]);

    const view = el(`
      <div>
        <div class="top-bar">
          <h1>${esc(entity.entity_type)}</h1>
          <button class="secondary" id="logout-button">Sign out</button>
        </div>
        <p><a href="#/entities">&larr; Back to entities</a></p>

        <dl>
          <dt>ID</dt><dd><code>${esc(entity.id)}</code></dd>
          <dt>Entity type</dt><dd>${esc(entity.entity_type)}</dd>
          <dt>Schema version</dt><dd>${esc(String(entity.schema_version))}</dd>
          <dt>Created</dt><dd>${new Date(entity.created_at).toLocaleString()}</dd>
          <dt>Updated</dt><dd>${new Date(entity.updated_at).toLocaleString()}</dd>
        </dl>

        <div class="top-bar">
          <h2>Data</h2>
          ${editing ? "" : `<button class="secondary" id="edit-button">Edit</button>`}
        </div>
        ${
          editing
            ? `
              <form id="edit-form">
                <textarea name="data" rows="12">${esc(JSON.stringify(entity.data, null, 2))}</textarea>
                ${editError ? `<p class="error">${esc(editError)}</p>` : ""}
                <div class="top-bar" style="justify-content: flex-start; gap: 0.5rem;">
                  <button type="submit">Save</button>
                  <button type="button" class="secondary" id="cancel-edit-button">Cancel</button>
                </div>
              </form>
            `
            : `<pre>${esc(JSON.stringify(entity.data, null, 2))}</pre>`
        }

        <h2>Relations</h2>
        ${context ? renderRelationsList(context.relations) : `<p class="hint">Relations unavailable.</p>`}
      </div>
    `);

    view.querySelector("#logout-button").addEventListener("click", () => {
      clearSession();
      location.hash = "#/login";
    });

    if (editing) {
      view.querySelector("#cancel-edit-button").addEventListener("click", () => {
        renderEntityDetailRoute(id);
      });
      view.querySelector("#edit-form").addEventListener("submit", async (event) => {
        event.preventDefault();
        const form = new FormData(event.target);
        let data;
        try {
          data = JSON.parse(form.get("data"));
        } catch {
          renderEntityDetailRoute(id, { editing: true, editError: "data must be valid JSON" });
          return;
        }
        try {
          await updateEntity(session.apiKey, id, data);
          renderEntityDetailRoute(id);
        } catch (err) {
          renderEntityDetailRoute(id, { editing: true, editError: err.message });
        }
      });
    } else {
      view.querySelector("#edit-button").addEventListener("click", () => {
        renderEntityDetailRoute(id, { editing: true });
      });
    }

    mount(view);
  } catch (err) {
    mount(
      el(`<p class="error">${esc(err.message)}</p><p><a href="#/entities">&larr; Back to entities</a></p>`),
    );
  }
}

function renderWorkspaceDetail(detail) {
  const view = el(`
    <div>
      <div class="top-bar">
        <h1>${esc(detail.name)}</h1>
        <button class="secondary" id="logout-button">Sign out</button>
      </div>
      <p><a href="#/dashboard">&larr; Back to workspaces</a></p>

      <div class="stat-grid">
        <div class="stat"><div class="value">${detail.entity_count}</div><div class="label">entities</div></div>
        <div class="stat"><div class="value">${detail.relation_count}</div><div class="label">relations</div></div>
        <div class="stat"><div class="value">${detail.schema_count}</div><div class="label">schemas</div></div>
      </div>

      <dl>
        <dt>Workspace ID</dt><dd><code>${detail.id}</code></dd>
        <dt>Max entities</dt><dd>${detail.max_entities ?? "unlimited"}</dd>
        <dt>Created</dt><dd>${new Date(detail.created_at).toLocaleString()}</dd>
      </dl>

      <button class="danger" id="delete-workspace-button">Delete workspace</button>
      <p class="error" id="delete-error" hidden></p>
    </div>
  `);

  view.querySelector("#logout-button").addEventListener("click", () => {
    clearSession();
    location.hash = "#/login";
  });

  view.querySelector("#delete-workspace-button").addEventListener("click", async () => {
    const confirmed = confirm(
      `Delete workspace "${detail.name}"? This permanently deletes every entity, relation, and schema in it.`,
    );
    if (!confirmed) {
      return;
    }
    const session = getSession();
    try {
      await deleteWorkspace(session.apiKey, detail.id);
      location.hash = "#/dashboard";
    } catch (err) {
      const errorEl = view.querySelector("#delete-error");
      errorEl.textContent = err.message;
      errorEl.hidden = false;
    }
  });

  return view;
}

async function renderWorkspaceDetailRoute(id) {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const detail = await getWorkspace(session.apiKey, id);
    mount(renderWorkspaceDetail(detail));
  } catch (err) {
    mount(el(`<p class="error">${err.message}</p><p><a href="#/dashboard">&larr; Back</a></p>`));
  }
}

function renderMembersTable(members) {
  const rows = members
    .map(
      (member) => `
        <tr>
          <td>${esc(member.email)}</td>
          <td>${esc(member.display_name ?? "")}</td>
          <td>${esc(member.role)}</td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>Email</th><th>Name</th><th>Role</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

function renderDashboardShell(overview, addMemberError) {
  const view = el(`
    <div>
      <div class="top-bar">
        <h1>Tenant Dashboard</h1>
        <button class="secondary" id="logout-button">Sign out</button>
      </div>
      <p class="hint">Tenant ${esc(overview.tenant_id)} &middot; plan: ${esc(overview.plan ?? "self-hosted / unmetered")}</p>

      <div class="stat-grid">
        <div class="stat"><div class="value">${overview.usage.workspace_count}</div><div class="label">workspaces${overview.max_workspaces != null ? ` / ${overview.max_workspaces}` : ""}</div></div>
        <div class="stat"><div class="value">${overview.usage.member_count}</div><div class="label">members</div></div>
        <div class="stat"><div class="value">${overview.usage.entity_count}</div><div class="label">entities</div></div>
      </div>

      <h2>Members</h2>
      ${renderMembersTable(overview.members)}

      <h2>Add a member</h2>
      <p class="hint">The person must already have an account (created via /auth/signup from an invite).</p>
      <form id="add-member-form">
        <label>Email
          <input type="email" name="email" required>
        </label>
        <label>Role
          <select name="role">
            <option value="viewer">Viewer</option>
            <option value="member" selected>Member</option>
            <option value="admin">Admin</option>
            <option value="owner">Owner</option>
          </select>
        </label>
        ${addMemberError ? `<p class="error">${addMemberError}</p>` : ""}
        <button type="submit">Add member</button>
      </form>
    </div>
  `);

  view.querySelector("#logout-button").addEventListener("click", () => {
    clearSession();
    location.hash = "#/login";
  });

  view.querySelector("#add-member-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const session = getSession();
    const form = new FormData(event.target);
    try {
      await addMember(session.apiKey, {
        email: form.get("email"),
        role: form.get("role"),
      });
      await renderDashboard();
    } catch (err) {
      mount(renderDashboardShell(overview, err.message));
    }
  });

  return view;
}

async function renderDashboard() {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const overview = await fetchTenantOverview(session.apiKey);
    mount(renderDashboardShell(overview));
  } catch (err) {
    if (err.status === 404) {
      // Not a session failure -- this deployment is yorishiro-server (community edition),
      // which has no /hosted/tenant/overview endpoint. The session (and the API key login
      // just issued) is still valid.
      mount(await renderLoginComplete(session));
      return;
    }
    clearSession();
    mount(renderLogin(`session expired: ${err.message}`));
  }
}

async function router() {
  const hash = location.hash || "#/login";

  const workspaceMatch = hash.match(/^#\/workspaces\/([0-9a-f-]+)$/i);
  if (workspaceMatch) {
    renderWorkspaceDetailRoute(workspaceMatch[1]);
    return;
  }

  if (hash === "#/dashboard") {
    renderDashboard();
    return;
  }

  if (hash === "#/schemas") {
    renderSchemasRoute();
    return;
  }

  const schemaMatch = hash.match(/^#\/schemas\/(.+)$/);
  if (schemaMatch) {
    renderSchemaDetailRoute(decodeURIComponent(schemaMatch[1]));
    return;
  }

  if (hash === "#/entities") {
    renderEntitiesRoute();
    return;
  }

  const entityMatch = hash.match(/^#\/entities\/([0-9a-f-]+)$/i);
  if (entityMatch) {
    renderEntityDetailRoute(entityMatch[1]);
    return;
  }

  const status = await checkSetupStatus();
  if (status.setup_required && hash !== "#/setup") {
    location.hash = "#/setup";
    return;
  }
  if (!status.setup_required && hash === "#/setup") {
    location.hash = "#/login";
    return;
  }

  mount(hash === "#/setup" ? await renderSetup() : renderLogin());
}

window.addEventListener("hashchange", router);
window.addEventListener("DOMContentLoaded", router);
