# External contract checks

`tools.json` records the reviewed CE tool inventory and EE additions.
The real MCP discovery scenario requires the selected edition's inventory to match exactly; runner-only checks compare both inventories to the source handlers.
Reports attribute successful and rejected MCP calls to their executing scenario without recording arguments or returned secrets.
The report verifier writes `tool-coverage.json`, listing each tool's observed outcomes and every missing successful call.
An observed successful call is not proof of complete behavior coverage, and each edition reports its missing successful calls explicitly.

Requires Python 3.10+ and an already-built application binary for the selected edition.
SQLite suites need no Python dependencies, external service, model download or licence.
The `tenant-postgres` suite additionally requires an owned PostgreSQL database and a valid enterprise licence.
The runner reads the repository's `config/test.yaml` and copies it into its owned temporary working directory.
Run the Make targets from the repository root, or use `make -C`.

```sh
make test-contract BINARY=/absolute/path/to/yorishiro SUITE=rest EDITION=ce
make test-contract BINARY=/absolute/path/to/yorishiro SUITE=mcp EDITION=ce
make test-contract BINARY=/absolute/path/to/yorishiro SUITE=management EDITION=ce
make test-contract BINARY=/absolute/path/to/yorishiro-ee SUITE=mcp EDITION=ee
```

Common contracts run on CE/SQLite; enterprise-only composition and behavior belong in explicit EE checks, not a duplicate common matrix.
The edition argument labels evidence; it does not infer or verify the binary's compile-time features.
The EE MCP delta suite checks edition discovery and entity-column and worker-class setting, IAM denial, nonmutation and reset through both transports, without replaying CE business scenarios.
The SQLite suites run with the enterprise licence explicitly unset; `tenant-postgres` requires a valid licence.
The EE provider scenario configures and resets both LLM and embedding overrides, checks secret-free REST/MCP metadata parity and preserves settings after invalid or unauthorized writes.
It uses reserved invalid provider URLs without starting workers, so it proves configuration and enqueue admission, not external inference or reindex completion.
The separate EE search scenario owns a loopback OpenAI-compatible HTTP stub and a real embedding worker, waits for reindex completion, and checks REST/MCP query results and cross-workspace isolation.
Its deterministic vectors test transport and worker integration, not semantic model quality or a live provider.
The EE composition scenario adopts a library template, changes its upstream definition, previews and applies a merge, and checks immutable schema history.
It then creates a second-workspace fork, checks scoped visibility and stale-head rejection, updates and deletes the fork without changing source history.

Each SQLite invocation migrates separate owned application and queue databases and bootstraps an account through the setup API.
Each invocation starts one server and reuses it for the selected suite.
The server starts separately from job workers.
The MCP suite additionally starts a separate `--environment test start --worker=job-target:shared --no-banner` process only after import submission and queued/empty-destination assertions.
The worker uses the server's exact environment, application database and separate queue database.
Children receive an allowlisted environment, an owned home directory, and embedding provider `none`.
The owned config disables auto-migration after explicit migration, and the auth rate limit is raised for these isolated checks.
The runner bounds readiness, commands, HTTP bodies and total suite duration; timeout values live in `run.py`.
JSON and SSE bodies are read in bounded available-byte chunks with a decreasing socket timeout, so trickled partial lines cannot extend the response deadline.
Import completion checks server/worker liveness and rejects unsuccessful job states within the suite deadline.
Worker cleanup runs on success, timeout and failure, with suite teardown as a fallback.
Cleanup terminates the process group, escalates to kill after five seconds, waits, and deletes owned state.

## Implemented coverage

The separate `tenant-postgres` EE suite requires an explicitly owned PostgreSQL URL and signed licence, and checks that authenticated cross-tenant invite acceptance is rejected without consuming the token, adding a membership or changing the original credential selection.
The required census assigns CE tools to CE suites except `search_entities` and `accept_invite`, which belong to the owned provider and licensed PostgreSQL EE suites.
EE owns all enterprise additions; each edition fails verification if an owned tool lacks a successful attributed call, except `accept_invite`, which requires a rejected call because a valid attributed credential already requires tenant membership.
This partitions all registered tools without replaying the full CE suite under EE, and does not claim exhaustive behavior coverage.
Its required GHA job owns a non-superuser PostgreSQL service and its own database; ordinary SQLite suites never inherit that database or licence.
The MCP migration lifecycle checks drift and a read-only dry-run, queues fill-defaults before starting a real worker, compares completed status through REST, and restores the original snapshot through queued undo.

Every suite bootstraps its own credentials; the MCP suite alone checks revocation and immediate rejection through both surfaces using the existing server and MCP session.

| Suite | Additional checks |
| --- | --- |
| `rest` | Schema create/read/archive/delete and entity create/list/read/update/delete through REST, cross-checked through MCP; unauthenticated/invalid credentials, IAM denial, malformed entity input |
| `mcp` | Same CRUD through MCP, cross-checked through REST; initialize/session header/initialized notification, tool discovery, authentication protocol errors, IAM denial tool errors, invalid UUID argument-deserialization tool error, missing schema arguments |
| `management` | CLI bootstrap/key tasks, argument and exit contracts, human key management, MCP profile/settings/template/workspace/audit/membership lifecycles |

Scenario definitions and assertions live in `run.py`.
The relation/export/import workflow submits through both transports before starting a worker, then checks completion and destination ownership without modifying the source.
Imported relations are active even when exported as deprecated; this is not lossless history restoration.
No database files are inspected or written directly by the tests.

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
python3 tests/contract/run.py --suite rest --edition ce --list
python3 tests/contract/run.py --self-check
```

`--list` prints a JSON array of the exact expected scenario IDs.
`--self-check` rejects empty/duplicate/invalid IDs and non-callable scenario definitions.

Runner-only checks use stdlib unittest and do not launch the application:

```sh
make test-python
```

GHA and local checks use these same Make targets.

## Limits

The public contracts cover selected workflows, not exhaustive route behavior, semantic search quality or concurrency safety.
PostgreSQL isolation, import replay/rollback, quotas, provider faults and races retain dedicated Rust tests.
MCP discovery asserts the reviewed complete inventory and correlates two concurrent responses on one session; it is not a load test.
It supports JSON and POST-response SSE with matching JSON-RPC IDs, not resumable server-push streams.
The temporary loopback port is selected by binding port zero and releasing it before startup; a competing bind causes an explicit startup failure rather than a retry against another port.
Cleanup cannot recover from SIGKILL of the Python runner itself.
