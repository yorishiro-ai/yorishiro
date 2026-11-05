# External black-box checks

`tools.json` records the reviewed CE tool inventory and EE additions.
The real MCP discovery scenario requires the selected edition's inventory to match exactly; runner-only checks compare both inventories to the source handlers.
Reports attribute successful and rejected MCP calls to their executing scenario without recording arguments or returned secrets.
The report verifier writes `tool-coverage.json`, listing each tool's observed outcomes and every missing successful call.
An observed successful call is not proof of complete behavior coverage, and each edition reports its missing successful calls explicitly.

`rest.builtin-template-relations` creates every builtin template's entity and relation types through public HTTP using Write credentials after schema creation.
The graph scenarios also cover Read-scoped MCP entity filtering, invalid relation-list status and missing-entity diagnostics.
Their internal predecessors were retired after the expanded replacement passed GHA run `41735414722` at commit `d5e1982291eda5a4bbcdad29790af5d699f2e60f`.

Requires Python 3.10+ and an already-built SQLite-capable application binary.
No Python dependencies, database access library, network embedding provider, model download, licence, or running external service is needed.
The runner reads the repository's `config/test_sqlite.yaml` and copies it into its owned temporary working directory.
Run from any directory, retaining this repository checkout beside the runner.

```sh
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro --suite rest --edition ce --artifacts /absolute/path/to/artifacts
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro --suite mcp --edition ce --artifacts /absolute/path/to/artifacts
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro --suite cli --edition ce --artifacts /absolute/path/to/artifacts
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro-ee --suite mcp --edition ee --artifacts /absolute/path/to/ee-artifacts
```

Common contracts run on CE/SQLite; enterprise-only composition and behavior belong in explicit EE checks, not a duplicate common matrix.
The edition argument labels evidence; it does not infer or verify the binary's compile-time features.
The EE MCP delta suite checks edition discovery and entity-column and worker-class setting, scope denial, nonmutation and reset through both transports, without replaying CE business scenarios.
Both editions run with the enterprise licence explicitly unset; licensed capabilities need separate ownership.

Each invocation runs public `db migrate` and CLI tenant/workspace/key creation against separate owned file-backed SQLite application and queue databases.
The CLI suite first verifies that a real work command fails timezone preparation before migration, then proves migration and normal tasks succeed.
It starts one server and reuses it for the whole suite.
Loco 1.2.0 defaults `start --no-banner` to server-only mode.
REST and MCP additionally start a separate `--environment test_sqlite start --worker=job-target:shared --no-banner` process only after import submission and queued/empty-destination assertions.
The worker uses the server's exact environment, application database and separate queue database.
Children receive an allowlisted environment, an owned home directory, and embedding provider `none`.
The owned config disables auto-migration after explicit migration, and the auth rate limit is raised for these isolated checks.
The server has a 45-second authenticated readiness deadline, individual commands have a 30-second deadline, HTTP operations have timeouts and body limits, and the suite has a 240-second budget checked between operations.
JSON and SSE bodies are read in bounded available-byte chunks with a decreasing socket timeout, so trickled partial lines cannot extend the response deadline.
Import completion has a strict 60-second budget within the suite deadline, checks server/worker liveness, and rejects unsuccessful job states.
Worker cleanup runs on success, timeout and failure, with suite teardown as a fallback.
Cleanup terminates the process group, escalates to kill after five seconds, waits, and deletes owned state.

## Implemented coverage

These scenarios cover selected workflows, not the entire advertised MCP surface.
Public coverage remains incomplete for search/recall, schema fork/merge, broader membership/tenant administration and explicit EE settings workflows.
The MCP migration lifecycle checks drift and a read-only dry-run, queues fill-defaults before starting a real worker, compares completed status through REST, and restores the original snapshot through queued undo.
Scenario counts alone are not a public-tool completeness guarantee.

Every suite checks CLI key issuance, REST and MCP acceptance, CLI revocation, and immediate rejection by both surfaces using the existing server and MCP session.

| Suite | Additional checks |
| --- | --- |
| `rest` | Schema create/read/archive/delete and entity create/list/read/update/delete through REST, cross-checked through MCP; unauthenticated/invalid credentials, insufficient scope, malformed entity input |
| `mcp` | Same CRUD through MCP, cross-checked through REST; initialize/session header/initialized notification, tool discovery, authentication protocol errors, scope tool errors, invalid UUID argument-deserialization tool error, missing schema arguments |
| `cli` | Registered bootstrap/key tasks, unknown flag/task, missing/invalid/malformed arguments, success/failure exits and diagnostics, UUID and one-time credential stdout, key listing without plaintext, revoked ID retained in listing and REST rejection |

`cli.human-key-management` creates an Owner invite through the CLI, accepts it through public signup and issues a user-attributed Schema key through the CLI.
REST and MCP identity readback must name that human.
An expiring delegated Read key cannot raise scope, add audit, omit its expiry ceiling or extend it through either transport; an unattributed service key cannot delegate management credentials.
MCP rotation must preserve grant metadata, invalidate the predecessor through both transports and retain its successor link in secret-free REST/MCP history.
REST revocation invalidates the successor through both transports and retains terminal history; a second rotation of the predecessor returns HTTP 409.
These sequential contracts supplement, not replace, the internal issuance/revocation race and deterministic expiry tests.

Schemas are versioned rather than updated in place.
Both CRUD scenarios reject active-schema deletion, create an evolved version, check its incremented version and active selection, and retain the historical definition.
Both reject stale entity updates through REST and MCP and verify unchanged data.
The human management scenario also creates, renames, caps, grants/revokes membership and deletes a workspace, checking list/get visibility and preservation of the other workspace's credential.
The shared fixture is an inline schema with one required string field.
The additional `rest.relation-export-import-worker` and `mcp.relation-export-import-worker` scenarios share one implementation with thin transport adapters.
Each creates a dedicated source workspace through the public CLI, a `graph` schema with `person.name` and `knows`, and Alice/Bob entities through its selected surface.
Relation create/read/list/delete and active/deprecated/archived status changes are read back through both surfaces, including omitted properties defaulting to `{}`, read-scope rejection, missing authentication, invalid status and missing endpoints.
Export while deprecated must return exactly one schema, two entities and one relation in kind order, preserving IDs/data/properties/status, with identical raw REST NDJSON and MCP text.
A new CLI-created destination workspace and schema service key submit the raw document through the selected surface before the real worker starts.
Both surfaces must report queued with an empty destination, then completed with job name `import` and counts 1/2/1.
Restored records must have new IDs, remapped schema/endpoints, destination ownership and matching data/properties, while the source export stays unchanged.
Imported relations are active even when exported as deprecated; this is not lossless history restoration.
No database files are inspected or written directly by the tests.

### Retired request tests

The request tests `relations_are_created_listed_statused_exported_and_deleted_over_rest` and `relation_write_requires_write_scope_and_read_authentication` were retired after successful GHA run `41693102412`, with replacement coverage in both `rest.relation-export-import-worker` and `mcp.relation-export-import-worker`, including Write-scoped relation status changes and deletion.
GHA run `41735414722` at commit `d5e1982291eda5a4bbcdad29790af5d699f2e60f` also proved the following replacements before retirement:

| Retired Rust test | Retained black-box assertions |
| --- | --- |
| `requests::relations::every_builtin_relation_type_can_be_created` | `rest.builtin-template-relations` creates each builtin schema and every entity/relation type with required fixture data, separate endpoints for same-type relations, Schema-scoped schema creation and Write-scoped entity/relation creation, asserting HTTP 201 throughout |
| `requests::mcp::community_mcp_entities_relations_and_schema_tools_execute_over_protocol` | `mcp.relation-export-import-worker` creates the graph schema, two entities and a relation, lists exactly two `person` entities with Read scope, checks invalid relation-list status and missing-entity tool errors and their diagnostics, and persists archived relation status for Read-scoped readback; successful MCP calls reject tool errors |

Existing export limits, history, security and race tests remain internal.

## Evidence and runner checks

Exit status is zero only when all selected assertions and cleanup pass.
The aggregate-verifier report is `<artifacts>/results.json`, containing `suite`, `edition`, `binary_sha256`, and `results` with each stable scenario `id`, `status`, and `duration_seconds`.
An unexecuted scenario is reported as failed with a zero duration, never silently omitted.
Additional artifacts are written under `<artifacts>/<edition>-<suite>/`: `transcript.json`, separate `server.stdout.log` and `server.stderr.log`, and `failure.log`.
Separate redacted `worker.stdout.log` and `worker.stderr.log` capture the import worker when started.
CLI stdout and stderr remain separate fields in the transcript.
Credentials are redacted before artifact writes, including credential prefixes and credentials issued immediately before a parsing failure.
Raw child logs exist only in the private temporary directory and are removed during teardown.
Use a distinct artifact root for each suite/edition invocation; repeated runs overwrite those named evidence files.

Discovery and definition checks never start a process and require no binary:

```sh
python3 tests/blackbox/run.py --suite rest --edition ce --list
python3 tests/blackbox/run.py --self-check
```

`--list` prints a JSON array of the exact expected scenario IDs.
`--self-check` rejects empty/duplicate/invalid IDs and non-callable scenario definitions.

Runner-only checks use stdlib unittest and do not launch the application:

```sh
python3 -m unittest discover -s tests/blackbox -p 'test_*.py'
python3 -m py_compile tests/blackbox/run.py tests/blackbox/test_runner.py
```

Application and unittest execution are reserved for CI for this batch.
Static syntax/lint checks may run locally.

## Limits

These additive batches do not cover PostgreSQL/RLS, enterprise-only features, embedding/search quality, concurrent writes, pagination boundaries, schema version evolution, or comprehensive route/task inventory.
Import replay/rollback, quotas, provider faults and races remain with the existing Rust tests; none are retired by this slice.
MCP discovery asserts a required subset, not a frozen complete inventory.
It supports JSON and POST-response SSE with matching JSON-RPC IDs, not resumable server-push streams.
The temporary loopback port is selected by binding port zero and releasing it before startup; a competing bind causes an explicit startup failure rather than a retry against another port.
Cleanup cannot recover from SIGKILL of the Python runner itself.
