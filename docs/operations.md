# Operational Notes

**English** | [日本語](ja/operations.md)

Yorishiro itself does not automate the concerns below; operators need to set these up separately.

## Backup and restore

Data lives entirely in PostgreSQL (in the development environment, the named volume `pgdata` in `docker-compose.yml`).
Docker Compose has no explicit project name set in this repo, so it prefixes the volume with the checkout directory's basename by default (e.g. `<dir>_pgdata`): run `docker compose config` to confirm the resolved name for your checkout.
Yorishiro has no built-in backup automation.

Set up scheduled backups with standard `pg_dump`/`pg_restore`, or a WAL-archiving + PITR (Point-in-Time Recovery) setup, on the operator side.
Relying on volume snapshots alone can produce an inconsistent backup.

## Rate limiting

There is currently no per-API-key or per-tenant *rate* limiting; request throughput isn't capped anywhere.
A single API key making heavy use of embedding generation or search can delay other requests.

This is especially true for `YORISHIRO_EMBEDDING_PROVIDER=local` (local ONNX inference), which serializes inference behind a single mutex.
Embedding generation for other tenants can be blocked too, not just the same tenant.
Introduce per-API-key rate limiting at a reverse proxy layer (nginx, Envoy, etc.) if needed.

Separately, there *is* a resource-count quota mechanism, not a rate limit: a tenant's `max_workspaces` and a workspace's `max_entities` are enforced at creation time.
Both default to `NULL` (unlimited), so a self-hosted deployment sees no caps unless an operator explicitly sets one via `admin create-tenant --max-workspaces`/`admin create-workspace --max-entities`.

This bounds how large a tenant/workspace can grow, but does nothing to smooth out request rate.
The two mechanisms are complementary, not substitutes for each other.

## Observability

Failures in embedding sync (background processing after an entity write) are currently only emitted to `tracing` logs (`RUST_LOG`).
There is no integration with a metrics backend.

If you need continuous monitoring, set up alerting on your log aggregation platform (Loki, CloudWatch Logs, etc.) and additionally run `admin resync-embeddings` periodically to check for anything missed.

## Access logging

Every request produces one JSON log line (method, path, status, latency) alongside the rest of the application's `tracing` output.
`YORISHIRO_LOG_TARGET` controls where all of it goes: see [configuration.md](configuration.md#logging).

- `stdout` is the right choice for a container runtime that collects logs from the process's standard streams.
- `single`/`daily` suit running the binary directly on a host without a surrounding log collector.
- `syslog` hands lines off to whatever the host's syslog daemon is already configured to do with them (forward, rotate, aggregate): Unix only; rejected at startup on other platforms.

None of these targets rotate or prune on their own beyond what `daily`'s day-boundary split does.
Pair `single`/`daily` with `logrotate` or an equivalent if disk usage needs to be bounded.

## Changing the embedding model

Vectors from two different models cannot share an HNSW index, and the server refuses to start when `YORISHIRO_EMBEDDING_DIMENSIONS` disagrees with the model it loaded: a mismatch stops the process rather than quietly returning bad search results.

An existing deployment is unaffected by a change of default: the dimension is read from the environment, so one already running 768 keeps its model and its vectors.
To move to a different model, re-embed:

```console
$ # 1. Stop the server, then replace models/model.onnx and models/tokenizer.json.
$ # 2. Set YORISHIRO_EMBEDDING_DIMENSIONS to the new model's width.
$ # 3. Clear the existing vectors: they belong to the old model.
$ psql "$DATABASE_URL" -c "UPDATE content.entities SET embedding = NULL"
$ # 4. Start the server, then regenerate per workspace:
$ yorishiro-server admin resync-embeddings <workspace-id>
```

Search still works between steps 3 and 4: entities without an embedding are reachable through the `pg_trgm` fallback, so the window degrades results rather than emptying them.

Re-embedding is the whole corpus through the model, so time it against a batch write rather than a query.

## Maintenance mode

Two modes, both shared by every node in the deployment (the state is a row in the database, not a flag in the process: a flag would put one replica in maintenance while its siblings kept serving):

| Mode | Reads | Writes | Status |
|---|---|---|---|
| `read-only` | served | refused | `423 Locked` |
| `full-lock` | refused | refused | `503 Service Unavailable` |

Both send `Retry-After`.
Agents retry on the header rather than on the body, so a refusal without one invites the immediate retry the mode exists to prevent.

```console
$ yorishiro-server admin maintenance read-only --retry-after 60 --reason "migrating schemas"
$ yorishiro-server admin maintenance-status
$ yorishiro-server admin maintenance off
```

`--reason` is shown to callers in place of the generic message; an operator saying "restoring from backup, back by 09:00" answers the question a bare status code provokes.

The same switch over REST, for an operator with no shell on the host:

```console
$ curl -X PUT http://localhost:8080/api/system/maintenance \
    -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
    -d '{"mode":"read-only","retry_after":60,"reason":"migrating schemas"}'
$ curl http://localhost:8080/api/system/maintenance -H "Authorization: Bearer $KEY"
```

Both need a `migration`-scoped key, the same scope that guards batch migration: each acts on data already stored, where a schema registration only adds a version nothing has been written against yet.
`retry_after` and `reason` are optional and mean what the CLI's flags mean.

`/up`, `/health` and `/api/system/maintenance` answer in every mode.
Refusing the probes would have an orchestrator restart a server that is deliberately paused, and a restart does not clear the state, so the loop would not converge.
Refusing the switch would make a full lock entered over REST leavable only over the CLI, which is a one-way door for whoever cannot reach a shell.

Read-only decides by HTTP method, so `POST /mcp` is treated as a write even when the tool called is a read: the middleware would have to consume the request body to know which tool it is, and a body consumed there is one the handler no longer has.
It errs toward refusing a read rather than admitting a write.

### Driving it from a monitor

The mode is a row, so anything holding the migration role's credentials can set it: the CLI is one caller, not the only one.
A monitor watching database load can shed writes on its own:

```sql
-- Sustained load: stop writes, tell callers when to come back.
UPDATE identity.maintenance
   SET mode = 'read_only', retry_after = 120,
       reason = 'database under sustained load', updated_at = now();

-- Recovered.
UPDATE identity.maintenance
   SET mode = 'off', reason = NULL, updated_at = now();
```

Every request already reads this row, so the change takes effect on the next request across every node: no restart, no deploy.

**Pair the entry with an exit.**
A monitor that switches to `read_only` and never switches back leaves the deployment refusing writes until somebody notices: an overnight spike becomes an outage that lasts until morning.
Whatever rule turns it on is the rule that has to turn it off.

The server does not watch load itself.
A database's CPU usage is not available over SQL, so anything the process could measure on its own (its own connection pool, `pg_stat_database`) measures demand rather than the load the threshold is about.

### Filling defaults

`POST /api/schemas/active/{name}/fill-defaults` (schema scope) writes the active version's `default` values into entities written before those fields existed, and returns a `job_id`.

Entities keep their own schema version.
Filling a value is not a migration between definitions: it adds data the entity was always allowed to hold, validated against the version it already claims.
What version an entity belongs to is a separate question.

A required field with **no** default is left alone and reported in `still_missing`.
A value nobody chose is indistinguishable from one somebody did, once written.

`POST /api/migration-jobs/{job_id}/undo` puts the whole run back.
The snapshots are consumed by the undo, so a job can only be undone once: a second undo would lay stale data over whatever came after the first.

### Inferring values

`fill-defaults` writes values the schema itself states; a required field with no default is reported in `still_missing` and left alone.
Inferring a value the schema does not state is a paid feature, `POST /api/schemas/active/{name}/infer-fill`, and the `yorishiro-ce` package does not carry it.

What decides that is the server making an outbound chat completion at all, not who pays for the call: a bring-your-own-key design moves the cost without changing the property.
The embedding providers are not an exception: an embeddings endpoint is not a chat completion, and the local ONNX provider makes no network call at all.

## Switching queue infrastructure

`DrainingQueue` runs two queues at once while an infrastructure switchover is in progress.
New work goes to the new queue from the moment it is installed; the old one keeps running what it had already accepted.

1. Construct `DrainingQueue::new(new, old)` and serve from it.
   Nothing upstream needs to know: it is a `Queue` like any other.
2. `drain_old(timeout)` waits for the old queue's outstanding work and **returns whether it finished**.
   `DrainOutcome::TimedOut` means something is still running.
   Do not proceed.
   This is deliberately not `drain()`: that one waits for *everything*, including work that has only just arrived on the new queue, which is not the question a switchover is asking.
3. Drop it and serve from the new queue directly.

Nothing sent through the switchover reaches the old queue: otherwise step 2 would be chasing a target that keeps moving.

## Chunked work

Long work is split into pieces that are acknowledged one at a time (`yorishiro_core::services::chunking`), so a worker that dies mid-job loses one chunk rather than the job.
`split` breaks at whitespace, never mid-word; `ChunkProgress` records which chunks were acknowledged and reports which were not.

**Reassignment is not here.**
Returning an unacknowledged chunk to the pool is a visibility timeout, a property of a queue that ships work between processes.
`LocalQueue` runs tasks on this runtime, where a lost chunk and a lost process are the same event.
A distributed driver brings its own timeout and reuses everything above: `outstanding()` is exactly the list it hands back.
