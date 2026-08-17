# Setup

**English** | [日本語](ja/setup.md)

## Prerequisites

The server needs an embedding model to start.
It defaults to the local ONNX provider, which needs no external service beyond the model files themselves.
But skipping this step isn't a degraded-mode startup: the process fails to start (before it even binds its listener) if `models/model.onnx`/`models/tokenizer.json` are missing, since neither the repository nor the Docker image bundles them.

1. Fetch the default model (multilingual-e5-large, 1024-dimensional, 100+ languages):

   ```console
   $ mkdir -p models
   $ curl -L -o models/model.onnx \
       https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/onnx/model_quantized.onnx
   $ curl -L -o models/tokenizer.json \
       https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/tokenizer.json
   ```

To use an OpenAI-compatible endpoint instead, see [embedding-providers.md](embedding-providers.md).

2. Provide **PostgreSQL 18 or newer**.
   The schema uses the built-in `uuidv7()` for primary keys, which arrived in 18; on an older server the migration fails outright rather than degrading.
3. Give the role in `DATABASE_URL` what the migration needs.
   It creates the extensions, the `yorishiro_app` application role, and every table, so:

   - `vector` (pgvector) must already be installed on the server.
     `pg_trgm` ships with PostgreSQL's contrib package, so a non-superuser can create it; pgvector does not, and installing it is a superuser (or packaging) step.
     If the role is not a superuser, create both extensions beforehand: either in the target database, or in `template1` so every database created afterwards inherits them.
     The migration declares both with `IF NOT EXISTS` and passes over extensions that already exist.
   - The role must be able to `SET ROLE yorishiro_app`, since that is the role the server serves requests as.
     PostgreSQL 16 and later do not grant that to the creating role automatically, so the migration issues `GRANT yorishiro_app TO CURRENT_USER` itself.
     A superuser may `SET ROLE` regardless of membership, which is why a superuser deployment never exercises this.

   - The role must be able to create schemas in the target database, since the migration declares `identity` and `content`.
     Owning the database is the usual way; otherwise it needs `GRANT CREATE ON DATABASE <name> TO <role>`.
     `CREATEDB` is not this: it permits creating *new* databases, not adding schemas to an existing one, so a role pointed at a database someone else owns fails with `permission denied for database`.

   Beyond those, a non-superuser needs `CREATEROLE` (to create `yorishiro_app`), plus `CREATEDB` if it creates the database itself.

Pick one of the three ways to run the server below.

## Run with Docker

The quickest way to start.
Needs Docker and a reachable PostgreSQL instance.

1. Complete [Prerequisites](#prerequisites) above.
2. Start the container, pointing it at your database and the model directory:

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -v "$(pwd)/models:/app/models:ro" \
       -e DATABASE_URL=postgres://... \
       ghcr.io/yotsunagi/yorishiro:latest
   ```

3. Confirm it's up:

   ```console
   $ curl localhost:8080/up
   ```

This is a complete, working single-tenant deployment as-is.
The web UI is compiled into the binary, so there's no separate `web/` directory to fetch or mount.
`YORISHIRO_MAX_TENANTS`/`YORISHIRO_EMBEDDING_PROVIDER` already default to single-tenant/local-ONNX, matching the volume mounted above.

See [configuration.md](configuration.md) to change any of that.
See [deployment.md](deployment.md#running-in-the-background) for running it in the background, building the image from source, or running the admin CLI from the same image.

## Install from a package

Every [release](https://github.com/yotsunagi/yorishiro/releases) attaches a `.deb` and a `.rpm` for `amd64` and `arm64`, in both editions.
There is no apt or yum repository to add: download the file and install it.

### Which package

The two conflict, so a machine has one or the other.

| Package | Files on the release page | What it is |
|---|---|---|
| `yorishiro-ee` | `yorishiro-ee_X.Y.Z_<arch>.deb`<br>`yorishiro-ee-X.Y.Z-1.<arch>.rpm` | The enterprise edition. Carries the paid features and the web UI; both stay inactive until `YORISHIRO_LICENSE_KEY` is set, so without a key it behaves exactly like the community edition. **Install this one** unless the row below applies to you. |
| `yorishiro-ce` | `yorishiro-ce_X.Y.Z_<arch>.deb`<br>`yorishiro-ce-X.Y.Z-1.<arch>.rpm` | The community edition, for a deployment that may not hold proprietary code at all. **Headless**: the web UI belongs to the paid edition, so this one serves nothing at `/`. The REST API, MCP server and admin CLI are identical. |

The edition is in the package name and nowhere else.
Both install `/usr/bin/yorishiro-server`, both ship `yorishiro.service`, and both read `/etc/yorishiro/`: so a runbook, a monitoring check or a `systemctl` command written for one works unchanged on the other.
`rpm -qi` reports the licence as `BUSL-1.1 AND LicenseRef-Yorishiro-EE` for the enterprise package and `BUSL-1.1` for the community one; Debian packages have no licence field, so there the package name and description carry it.

### Installing

```console
$ sudo dpkg -i yorishiro-ee_X.Y.Z_amd64.deb  # or: sudo rpm -i yorishiro-ee-X.Y.Z-1.x86_64.rpm
$ sudoedit /etc/yorishiro/config.yml         # at minimum, database_url
$ sudo systemctl enable --now yorishiro
```

The service runs as the `yorishiro` system user the package creates, with its state in `/var/lib/yorishiro`.

Enabling it before setting `DATABASE_URL` stops the unit at `failed` with `status=78/CONFIG`, and `journalctl -u yorishiro` names the file to edit.
It does not retry, because waiting does not supply a missing setting.
A database that is merely not up yet is the opposite case: that is retried every five seconds, so a server booting alongside its own PostgreSQL recovers on its own.

### Switching editions

Install the other package.
It replaces the one that is there:

```console
$ sudo dpkg -i yorishiro-ce_X.Y.Z_amd64.deb  # or: sudo rpm -U yorishiro-ce-X.Y.Z-1.x86_64.rpm
```

Nothing else is needed. `/etc/yorishiro/` and `/var/lib/yorishiro` belong to the deployment rather than to the edition, the unit keeps its name and whatever enabled state it had, and the binary at `/usr/bin/yorishiro-server` is swapped in place.
Restart the service to pick it up.

Moving to the community edition drops the web UI and the paid features; the database is untouched, so moving back restores them.

### Verifying a download

The packages are not GPG-signed.
Every release attaches a `checksums.txt` covering all eight files, so a download can still be checked against what was published:

```console
$ curl -LO https://github.com/yotsunagi/yorishiro/releases/download/vX.Y.Z/checksums.txt
$ sha256sum --check --ignore-missing checksums.txt
```

That confirms the file is the one published.
To confirm *where it came from* (which workflow built it, from which commit), each package also carries build provenance:

```console
$ gh attestation verify yorishiro-ee_X.Y.Z_amd64.deb --repo yotsunagi/yorishiro
```

No key to import.
The attestation is recorded in a public transparency log, and `gh` checks it against this repository.

### Supported systems

The packages require **glibc 2.38 or newer** (Ubuntu 24.04, Debian 13, Fedora 39 and later).
That floor comes from the ONNX Runtime the embedding provider links, not from Yorishiro itself.
It is declared as a package dependency, so apt and dnf refuse an older system rather than installing a binary that cannot start.

Both halves of that are tested on every pull request by installing the packages, not by reading them: `packaging/test-install.sh` runs them through Ubuntu 24.04 and Fedora 39 (glibc 2.38 exactly, the tightest system supported) and requires Ubuntu 22.04 and Rocky 9 to refuse by name.
`packaging/test-systemd.sh` covers what only systemd can exercise: that an unconfigured start stops instead of retrying, that a configured one serves `/up`, and that the service returns by itself after a reboot.

Both scripts take a directory of built packages and need Docker; the systemd one also needs a privileged container.
CI runs the same two, so a failure there reproduces locally.
To build the packages for it, run `nfpm package --config packaging/nfpm-yorishiro-ee.yaml --packager deb --target dist/` from the repository root (`nfpm` resolves `src:` against the working directory) and repeat for `--packager rpm` and `nfpm-yorishiro-ce.yaml`.
Build the binaries in a container unless the host's glibc is at or below the floor, since a newer one emits symbols the packages do not declare.

### Running the binary outside the package

The package is the supported way to install on bare metal or a VM: it is the same binary, plus the service user, the state directory and the systemd unit, and it declares the glibc floor so the package manager refuses a system that cannot run it.
There is no standalone tarball: a release attaches the eight packages (two editions, two architectures, two formats) and their checksums, and nothing else.

To run the binary from somewhere other than `/usr/bin`, take it out of the package (`dpkg-deb -x`, `rpm2cpio | cpio -id`) and put the `models/` directory from step 1 beside it.
The file to extract is `usr/bin/yorishiro-server`, whichever edition the package is.
Configure it with a `config.yml` in the directory the server runs from, or point `YORISHIRO_CONFIG_PATH` at one elsewhere (see [configuration.md](configuration.md#configyml) and [`config.example.yml`](../config.example.yml)), then start it.
[deployment.md](deployment.md#running-in-the-background) covers keeping it running across reboots when the package's own unit is not being used.

## Run from source (Docker Compose)

For local development.
Needs Docker, Docker Compose, and `make`.

1. Clone the repository and complete [Prerequisites](#prerequisites) above inside it:

   ```console
   $ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro
   # (place models/model.onnx and models/tokenizer.json as above)
   ```

2. Build the images (from the same multi-stage `Dockerfile` the release image is built from) and start PostgreSQL plus `app` (`docker-compose.yml` already points it at the local ONNX provider):

   ```console
   $ make init
   ```

Every `-e`/environment variable used by any of the three methods above can go in a `config.yml` file instead.
Mount it at `/app/config.yml` for Docker.
This is often more convenient than a long list of `-e` flags: see [configuration.md](configuration.md#configyml) and [`config.example.yml`](../config.example.yml).

## Endpoints

Migrations are applied automatically on startup, for all three methods above.

| Path | Description |
|---|---|
| `http://localhost:8080/up` | Liveness probe (always 200 if the process is running; no dependency checks) |
| `http://localhost:8080/health` | Readiness check (also probes DB connectivity; 503 on outage) |
| `http://localhost:8080/` | Web UI (compiled into the binary; see `YORISHIRO_WEB_DIR` in [configuration.md](configuration.md) to serve it from disk instead): see [Web UI](#web-ui) below for what it covers |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI document (REST API reference) |
| `http://localhost:8080/api-docs/openapi.json` | OpenAPI specification |
| `http://localhost:8080/mcp` | MCP endpoint (Streamable HTTP) |
| `http://localhost:8080/whoami` | Authentication check (returns workspace, tenant, and scope) |

## First-run setup

Deployments where `YORISHIRO_MAX_TENANTS` resolves to an actual cap (the default: unset means `1`) serve a setup wizard at `http://localhost:8080/`.
No admin CLI needed.

The wizard is served over HTTP, so a database has to be reachable before there is anything to serve it from.
A server started with no database configured exits and names the file to set it in.
Set the connection string, start the server, and the wizard covers everything after that.

On first visit, since no tenant exists yet, the browser shows a form for an email and password (plus an optional display name and an optional schema template to bootstrap the `default` workspace with).
Submitting it creates the tenant, its `default` workspace, and an owner account in one step, and displays the freshly issued API key (shown only once, same as every other key in this system).
Visiting the same page afterward shows a login form instead.

The same flow is available without a browser:

```console
$ curl localhost:8080/setup/status
{"setup_required":true}
$ curl -X POST localhost:8080/setup -H "Content-Type: application/json" \
    -d '{"email":"owner@example.com","password":"a strong password"}'
{"user_id":"...","email":"owner@example.com","tenant_id":"...","workspace_id":"...",
 "api_key":"ysr_..."}
```

`POST /setup` returns `409` once a tenant already exists, and `404` on any deployment where `YORISHIRO_MAX_TENANTS` resolves to unlimited (i.e. explicitly set to `0`).
Those deployments add tenants via signup/invite instead (see [Signup, login, member, and workspace management](#signup-login-member-and-workspace-management)).

The admin CLI below remains available for anything the wizard doesn't cover: additional workspaces/tenants, invites, and key rotation.

## Tenants, workspaces, and users

Yorishiro's control plane is two-tiered:

- A **tenant** is an organization/account.
  It can have `max_workspaces` set (a billing cap; `NULL`, the default, means unlimited, which is appropriate for self-hosted deployments).
  It can have any number of human **users** attached to it, each with a role (`owner` / `admin` / `member` / `viewer`) recorded in a membership.
  A user can belong to multiple tenants.
  **Schemas** are owned by a workspace: each workspace has its own, so editing one does not reach the others.
- A **workspace** belongs to exactly one tenant and is the actual operational container.
  Entities, relations, and API keys all scope to a workspace.
  A workspace references one schema (via `schema_id`) and can have `max_entities` set (also `NULL`/unlimited by default).

Splitting tenant from workspace lets one organization run several isolated projects (e.g. separate workspaces per environment or team) without provisioning a whole new tenant for each.
Schemas being per workspace means one workspace can change its structure without imposing that change on its siblings: two workspaces may even hold schemas of the same name at different versions.
It also lets several people share administrative access to the same tenant via memberships.

Tenant *creation* is only available through the admin CLI below, by whoever holds `DATABASE_URL`.
Workspace creation is available there too, but once a tenant has at least one owner/admin with an API key, they can also create additional workspaces over REST.
Day-to-day *membership* management (inviting/adding/listing members) is likewise available to tenant owners/admins over REST: see [Signup, login, member, and workspace management](#signup-login-member-and-workspace-management).

By default (unset `YORISHIRO_MAX_TENANTS`), a deployment is capped at a single tenant, so `admin create-tenant` and the signup flow below can never create a second one.
Set `YORISHIRO_MAX_TENANTS=0` (see [configuration.md](configuration.md)) to allow unlimited tenants, or to a specific number to allow that many.

## Provisioning tenants, workspaces, and API keys

Deployments that used the setup wizard above can skip this section for their first (and, under the default `YORISHIRO_MAX_TENANTS=1`, only) tenant.
It remains the only way to provision *additional* tenants/workspaces.
On deployments where `YORISHIRO_MAX_TENANTS` resolves to unlimited, the wizard is disabled, so this is the only way to provision anything at all.

API keys are stored in the database only as SHA-256 hashes and user passwords only as argon2 hashes, so neither can be provisioned by hand in SQL: both go through the admin CLI:

```console
$ make admin ARGS="create-tenant my-team"
tenant created
  id:            019f565d-f1e3-7afb-b876-b7003e43c230
  name:          my-team
  max_workspaces: unlimited

next steps:
  1. create a schema (via REST API or --template)
  2. admin create-workspace 019f565d-f1e3-7afb-b876-b7003e43c230 <name> --schema-id <id>

$ make admin ARGS="create-api-key <workspace-id> write"
api key created (the plaintext key is shown ONLY once, store it now)
  key:          ysr_928e48292888_ef72...
  ...

$ make admin ARGS="list-tenants"
```

`create-tenant <name> [--max-workspaces <n>] [--template <id>]` creates the tenant only, by default.
`--max-workspaces` caps the number of workspaces the tenant may create (omit for unlimited).
To start working, create a schema first, from a built-in template or from a definition of your own, either way through the REST API, MCP or the Web UI ("Create Custom Schema" on the schemas page), then create a workspace with `--schema-id` to link it.
Each workspace uses exactly one schema (1:1 relationship).
The plaintext API key is shown only once, at issuance time.

Pass `--template <id>` to `create-tenant` to bootstrap a schema from a built-in template and create a default workspace linked to it in the same step.
Without this flag, only a tenant is created (no workspace, no login possible).
Example: `admin create-tenant acme --template general-notes`.

Admin commands access the database directly using the connection role from `DATABASE_URL`.
This is the same administrative role used for migrations, and the only role permitted to touch `identity.tenants`/`identity.users`/`identity.tenant_memberships` at all: the application's own `yorishiro_app` role cannot.

Other admin commands:

| Command | Description |
|---|---|
| `admin create-tenant <name> [--max-workspaces <n>] [--template <id>]` | Create a tenant, optionally capping its workspace count and bootstrapping a schema/workspace from a template |
| `admin list-tenants` | List all tenants |
| `admin create-workspace <tenant-id> <name> [--schema-id <id>] [--max-entities <n>]` | Create an additional workspace under a tenant, optionally linking it to a schema |
| `admin list-workspaces <tenant-id>` | List a tenant's workspaces |
| `admin create-user <email> <password> [--display-name <name>]` | Create a human user account |
| `admin add-member <tenant-id> <user-id> <role>` | Add (or change the role of) a user's membership in a tenant (`owner`/`admin`/`member`/`viewer`) |
| `admin list-members <tenant-id>` | List a tenant's members and their roles |
| `admin create-invite <tenant-id> <email> <role> [--ttl-hours <n>]` | Issue an invite token for an email to join a tenant (default TTL: 7 days): see below |
| `admin create-api-key <workspace-id> <scope> [--user <user-id>]` | Issue an API key, optionally attributed to a member |
| `admin list-api-keys <workspace-id>` | List keys (ID, scope, prefix, attributed user, last used) |
| `admin revoke-api-key <key-id>` | Immediately revoke a key (e.g. on leakage) |
| `admin resync-embeddings <workspace-id>` | Re-sync entities missing an embedding (recovery from a failed sync) |

## Authentication and scopes

All APIs authenticate via `Authorization: Bearer <api-key>`.
Keys are strings starting with `ysr_`, shown only once at issuance time (only a SHA-256 hash is stored in the database).

Scopes form a four-level hierarchy: `read` < `write` < `schema` < `migration`.
Each subsumes the ones below it.
`migration` is required to run a batch migration or undo one: operations that rewrite rows already stored, where registering a schema only adds a version nothing has been written against yet.

### Attributing keys to users

Every request, human or automated, is ultimately authenticated by an API key: there's no cookie/session state on the server.
But a key can be *attributed* to a human user, and multi-user access control works by tying that attribution to the user's tenant role rather than by a session.

Passing `--user <user-id>` to `create-api-key` attributes the key to that member and caps the requested scope at `MembershipRole::max_scope()`: `owner`/`admin` may be issued up to `migration`, `member` up to `write`, and `viewer` up to `read`.

Requesting a scope above that cap, or attributing a key to someone who isn't a member of the workspace's tenant, is rejected at issuance time.

This check runs once, when the key is created.
Like a key's scope itself, it isn't re-evaluated on every request, so revoking a user's membership doesn't retroactively narrow keys already issued to them.
Revoke the key instead.

Omit `--user` for unattributed service/automation keys, which aren't capped by any role.
`GET /whoami` echoes the attributed `user_id` (or `null`) alongside the workspace, tenant, and scope.

`POST /auth/login` (below) is the self-service equivalent of `admin create-api-key --user`: it authenticates with a password instead of `DATABASE_URL` access, and issues a key already capped at the caller's own role.

## Signup, login, member, and workspace management

Account creation is invite-only: there is no public, unauthenticated signup.
A tenant owner/admin issues an invite (by CLI or, once they hold an API key, by REST), the invitee redeems it once to create their account, and from then on they authenticate with email/password to obtain API keys, rather than being handed one out of band.

1. Invite

   A tenant owner/admin creates an invite token for an email address and role:

   ```console
   $ make admin ARGS="create-invite 019f565d-f1e3-7afb-b876-b7003e43c230 newperson@example.com member"
   invite created (the plaintext token is shown ONLY once, send it now)
     token:      c8b9ea1f...
     ...
     expires at: 2026-07-20 16:57 UTC
   ```

   - Send the plaintext `token` to the invitee out of band (email, chat, etc.).
     Like an API key, it's shown only once and only its hash is persisted.
   - It expires after `--ttl-hours` (default 7 days) or immediately upon being redeemed, whichever comes first.

2. Signup

   The invitee redeems the token to create their account:

   ```console
   $ curl -X POST localhost:8080/auth/signup -H "Content-Type: application/json" \
       -d '{"invite_token":"c8b9ea1f...","password":"a strong password","display_name":"New Person"}'
   {"user_id":"...","email":"newperson@example.com","tenant_id":"...","role":"member",
    "workspaces":[{"id":"...","name":"default"}]}
   ```

   This both creates the `identity.users` row and adds the membership the invite specified.
   A second signup attempt with the same (now-consumed) token is rejected (422).

3. Login

   From then on, the user exchanges their password for a freshly issued API key, scoped to one workspace and capped at their role's `max_scope()` (see above).

   - `workspace_id` can be omitted: it's auto-resolved when the account has access to exactly one workspace, which is true for every deployment by default.
   - It only needs to be passed explicitly when the account belongs to more than one, in which case a 422 asks for it.
   - That 422's `details` carries one entry per candidate workspace: `field` is the workspace id, `problem` is its name, so a client can render a picker from it.

   ```console
   $ curl -X POST localhost:8080/auth/login -H "Content-Type: application/json" \
       -d '{"email":"newperson@example.com","password":"a strong password"}'
   {"api_key":"ysr_...","api_key_id":"...","workspace_id":"...","scope":"write","user_id":"..."}
   ```

   Every login issues a *new* key rather than reusing one.
   Revoke old ones with `admin revoke-api-key` if they're no longer needed.

4. Member management

   Once authenticated, a tenant owner/admin can list and add members over REST without needing `DATABASE_URL`/the admin CLI at all:

   ```console
   $ curl localhost:8080/api/members -H "Authorization: Bearer $YORISHIRO_KEY"
   $ curl -X POST localhost:8080/api/members -H "Authorization: Bearer $YORISHIRO_KEY" \
       -H "Content-Type: application/json" \
       -d '{"email":"existing-user@example.com","role":"admin"}'
   ```

   - `POST /api/members` attaches an *existing* account (one that already completed signup) to the caller's tenant.
     It never creates a new account.
     To bring in someone with no account yet, issue them an invite (step 1) instead.
   - Both endpoints require the caller's own key to be attributed to an Owner/Admin member.
     A Member-role key is rejected with 403 regardless of its own scope, since membership management is a tenant-role concern, not a scope one.

5. Workspace management

   Similarly, any authenticated member can list a tenant's workspaces (including their entity/relation/schema counts), while creating or deleting one is restricted to owners/admins, the same way member management is:

   ```console
   $ curl localhost:8080/api/workspaces -H "Authorization: Bearer $YORISHIRO_KEY"
   $ curl -X POST localhost:8080/api/workspaces -H "Authorization: Bearer $YORISHIRO_KEY" \
       -H "Content-Type: application/json" -d '{"name":"staging"}'
   $ curl localhost:8080/api/workspaces/$WORKSPACE_ID -H "Authorization: Bearer $YORISHIRO_KEY"
   $ curl -X DELETE localhost:8080/api/workspaces/$WORKSPACE_ID -H "Authorization: Bearer $YORISHIRO_KEY"
   ```

   - Deleting a workspace cascades to everything under it: entities, relations, schemas, API keys.
     It's rejected with 409 if it's the tenant's only remaining workspace, since there would be no way to provision a replacement without `DATABASE_URL` access.
   - The web UI (`/`) exposes the same create/list/delete/detail operations after signing in: see [Web UI](#web-ui) below.

## Web UI

Beyond first-run setup, login, and member/workspace management (above), the web UI also offers:

- **Schemas**: browse the tenant's registered schemas and their entity types, apply a built-in template, or register a definition of your own with "Create Custom Schema".
- **Entities**: browse, filter and page through a workspace's entities, create one from a schema-driven form, and open a detail view that shows its relations and edits the data payload.
- **Search and graph**: similarity search over a workspace, and an interactive graph of a schema's structure or an entity's relations.
- **Import and export**: JSON Lines in and out, per workspace.

The tenant's DB-backed template library (`/api/template-library`) has no page: it is reachable over the REST API and MCP only.

It is not a full data-management UI: the REST API (`/api-docs/openapi.json`) and MCP tools cover everything it doesn't.
