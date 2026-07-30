// Yorishiro admin dashboard -- a deliberately framework-free SPA. Scope is limited to first-run
// setup, login, usage/billing display, member management, and workspace management (create/
// list/delete plus a summary detail view -- entity/relation/schema counts, not their content);
// it is not a general entity/schema/relation browser -- that's what the REST API + Swagger UI
// (`/docs` on yorishiro-server) are for. Served by both yorishiro-server (via `YSR_WEB_DIR`) and
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
