# External black-box checks

Requires Python 3.10+ and an already-built SQLite-capable application binary.
No Python dependencies, database access library, network embedding provider, model download, licence, or running external service is needed.
The runner reads the repository's `config/test_sqlite.yaml` and copies it into its owned temporary working directory.
Run from any directory, retaining this repository checkout beside the runner.

```sh
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro --suite rest --edition ce --artifacts /absolute/path/to/artifacts
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro --suite mcp --edition ce --artifacts /absolute/path/to/artifacts
python3 tests/blackbox/run.py --binary /absolute/path/to/yorishiro --suite cli --edition ee --artifacts /absolute/path/to/artifacts
```

Repeat each suite with `--edition ce` and `--edition ee`, supplying the corresponding independently built binary.
The edition argument labels evidence; it does not infer or verify the binary's compile-time features.
Both editions exercise only the community surface, with the enterprise licence explicitly unset.

Each invocation runs public `db migrate` and CLI tenant/workspace/key creation against separate owned file-backed SQLite application and queue databases.
It starts one server and reuses it for the whole suite.
Loco 1.2.0 defaults `start --no-banner` to server-only mode, so no worker or scheduler process is requested.
Children receive an allowlisted environment, an owned home directory, and embedding provider `none`.
The owned config disables auto-migration after explicit migration, and the auth rate limit is raised for these isolated checks.
The server has a 45-second authenticated readiness deadline, individual commands have a 30-second deadline, HTTP operations have timeouts and body limits, and the suite has a 240-second budget checked between operations.
Cleanup terminates the process group, escalates to kill after five seconds, waits, and deletes owned state.

## Implemented coverage

Every suite checks CLI key issuance, REST and MCP acceptance, CLI revocation, and immediate rejection by both surfaces using the existing server and MCP session.

| Suite | Additional checks |
| --- | --- |
| `rest` | Schema create/read/archive/delete and entity create/list/read/update/delete through REST, cross-checked through MCP; unauthenticated/invalid credentials, insufficient scope, malformed entity input |
| `mcp` | Same CRUD through MCP, cross-checked through REST; initialize/session header/initialized notification, tool discovery, authentication protocol errors, scope tool errors, invalid UUID protocol error, missing schema arguments |
| `cli` | Registered bootstrap/key tasks, unknown flag/task, missing/invalid/malformed arguments, success/failure exits and diagnostics, UUID and one-time credential stdout, key listing without plaintext, revoke/list disappearance and REST rejection |

Schemas are versioned rather than updated in place, so this batch exercises creation, reads, archive and deletion, not schema version evolution.
The shared fixture is an inline schema with one required string field.
No database files are inspected or written directly by the tests.

## Evidence and runner checks

Exit status is zero only when all selected assertions and cleanup pass.
The aggregate-verifier report is `<artifacts>/results.json`, containing `suite`, `edition`, `binary_sha256`, and `results` with each stable scenario `id`, `status`, and `duration_seconds`.
An unexecuted scenario is reported as failed with a zero duration, never silently omitted.
Additional artifacts are written under `<artifacts>/<edition>-<suite>/`: `result.json`, `transcript.json`, separate `server.stdout.log` and `server.stderr.log`, and `failure.log`.
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

This first batch does not cover PostgreSQL/RLS, enterprise-only features, worker execution, embedding/search quality, concurrent writes, pagination boundaries, schema version evolution, or comprehensive route/task inventory.
MCP discovery asserts a required subset, not a frozen complete inventory.
It supports JSON and POST-response SSE with matching JSON-RPC IDs, not resumable server-push streams.
The temporary loopback port is selected by binding port zero and releasing it before startup; a competing bind causes an explicit startup failure rather than a retry against another port.
Cleanup cannot recover from SIGKILL of the Python runner itself.
