#!/usr/bin/env python3
"""External, dependency-free checks against an already-built Yorishiro binary."""

import argparse
import contextlib
import hashlib
import http.client
import json
import os
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time
import traceback
import uuid
from pathlib import Path

TIMEOUT = 30
MAX_BODY = 2 * 1024 * 1024
PROTOCOL = "2025-03-26"
ROOT = Path(__file__).resolve().parents[2]
SCHEMA = {
    "name": "blackbox",
    "entity_types": {"note": {"fields": {
        "title": {"type": "string", "required": True},
    }}},
    "relation_types": {},
}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def redact(text, secrets=()):
    for secret in sorted(filter(None, secrets), key=len, reverse=True):
        text = text.replace(secret, "[REDACTED]")
    text = re.sub(r"ysr_[A-Za-z0-9_-]+", "[REDACTED]", text)
    text = re.sub(r"(?i)(Bearer\s+)[^\s\"'\\,}]+", r"\1[REDACTED]", text)
    return text


def output_field(text, label, is_uuid=False):
    matches = re.findall(r"^" + re.escape(label) + r":\s*(\S+)\s*$", text, re.MULTILINE)
    require(len(matches) == 1, f"expected exactly one stdout field: {label}")
    value = matches[0]
    if is_uuid:
        require(str(uuid.UUID(value)) == value, f"noncanonical UUID: {label}")
    return value


def rpc_document(text, request_id):
    """Extract one matching JSON-RPC response from JSON or a completed SSE event."""
    if text.lstrip().startswith("{"):
        documents = [json.loads(text)]
    else:
        documents = []
        for event in re.split(r"\r?\n\r?\n", text):
            data = []
            for line in event.splitlines():
                if line.startswith("data:"):
                    value = line[5:]
                    data.append(value.removeprefix(" "))
            payload = "\n".join(data)
            if payload.strip():
                documents.append(json.loads(payload))
    matches = [doc for doc in documents if doc.get("id") == request_id]
    require(len(matches) == 1, f"expected one RPC response for id {request_id}")
    require(matches[0].get("jsonrpc") == "2.0", "missing JSON-RPC version")
    require(("result" in matches[0]) != ("error" in matches[0]), "invalid RPC envelope")
    return matches[0]


def tool_value(document):
    require("error" not in document, f"RPC error: {document}")
    result = document["result"]
    require(not result.get("isError", False), f"tool error: {result}")
    texts = [item["text"] for item in result.get("content", []) if item["type"] == "text"]
    require(len(texts) == 1, "expected one tool JSON text block")
    return json.loads(texts[0])


def tool_error(document):
    return "error" in document or document.get("result", {}).get("isError") is True


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--suite", choices=("rest", "mcp", "cli"))
    parser.add_argument("--edition", choices=("ce", "ee"))
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--list", action="store_true", help="print selected scenario IDs as JSON")
    parser.add_argument("--self-check", action="store_true", help="validate scenario definitions only")
    args = parser.parse_args(argv)
    if args.self_check:
        return args
    if args.suite is None or args.edition is None:
        parser.error("--suite and --edition are required")
    if args.list:
        return args
    if args.binary is None or args.artifacts is None:
        parser.error("--binary and --artifacts are required to execute scenarios")
    if not args.binary.is_absolute() or not args.binary.is_file():
        parser.error("--binary must name an existing absolute binary path")
    if not os.access(args.binary, os.X_OK):
        parser.error("--binary must be executable")
    args.artifacts = args.artifacts.resolve()
    return args


class App:
    def __init__(self, args, directory):
        self.args = args
        self.directory = Path(directory)
        self.secrets = []
        self.events = []
        self.server = None
        self.server_files = []
        self.logs = contextlib.ExitStack()
        self.sequence = 0
        self.started = time.monotonic()
        self.deadline = self.started + 240
        self.checks = []
        self.results = [{"id": name, "status": "failed", "duration_seconds": 0,
                         "detail": "not executed"} for name, _ in scenarios(args.suite)]
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        # Only these ambient values are needed to execute a prebuilt binary.
        self.env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT") if key in os.environ}
        home = self.directory / "home"
        home.mkdir()
        self.env.update({
            "HOME": str(home), "TMPDIR": str(self.directory), "LANG": "C.UTF-8",
            "LOCO_ENV": "test_sqlite", "NO_COLOR": "1", "RUST_BACKTRACE": "0",
            "DATABASE_URL": f"sqlite://{self.directory}/app.sqlite3?mode=rwc",
            "QUEUE_URL": f"sqlite://{self.directory}/queue.sqlite3?mode=rwc",
            "PORT": str(self.port), "BINDING": "127.0.0.1",
            "YORISHIRO_EMBEDDING_PROVIDER": "none",
            "YORISHIRO_LICENSE_KEY": "", "YORISHIRO_PLAN_POLICY_ENABLED": "false",
            "YORISHIRO_AUTH_RATE_LIMIT_MAX": "10000",
            "YORISHIRO_MCP_ALLOWED_HOSTS": "localhost,127.0.0.1",
        })
        config = self.directory / "config"
        config.mkdir()
        self.env["LOCO_CONFIG_FOLDER"] = str(config)
        text = (ROOT / "config/test_sqlite.yaml").read_text()
        require(text.count("auto_migrate: true") == 1, "shipped SQLite config changed")
        (config / "test_sqlite.yaml").write_text(text.replace("auto_migrate: true", "auto_migrate: false"))

    def remaining(self, cap=TIMEOUT):
        remaining = self.deadline - time.monotonic()
        require(remaining > 0, "suite deadline exceeded")
        return min(cap, remaining)

    def cli(self, *arguments, ok=True):
        command = [str(self.args.binary), "--environment", "test_sqlite", *arguments]
        # File-backed capture stays bounded by disk, not pipe buffers, and survives timeout cleanup.
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            process = subprocess.Popen(command, cwd=ROOT, env=self.env,
                                       stdout=stdout, stderr=stderr, start_new_session=True)
            try:
                process.wait(timeout=self.remaining())
            except subprocess.TimeoutExpired:
                self.stop(process)
                raise AssertionError(f"CLI timeout: {arguments[0]}") from None
            finally:
                if process.poll() is None:
                    self.stop(process)
                stdout.seek(0)
                stderr.seek(0)
                out = stdout.read().decode("utf-8", "replace")
                err = stderr.read().decode("utf-8", "replace")
                self.events.append({"cli": arguments, "exit": process.returncode,
                                    "stdout": out, "stderr": err})
        require((process.returncode == 0) == ok,
                f"unexpected CLI exit {process.returncode}: {arguments}")
        return out, err

    def task(self, task_name, **params):
        return self.cli("task", task_name, *(f"{key}:{value}" for key, value in params.items()))[0]

    def issue(self, workspace, scope):
        out = self.task("create_api_key", workspace_id=workspace, scope=scope)
        key = output_field(out, "api key (shown once)")
        self.secrets.append(key)
        require(key.startswith("ysr_"), "unexpected issued credential format")
        return output_field(out, "api key id", True), key

    def boot(self):
        self.cli("db", "migrate")
        tenant = output_field(self.task("create_tenant", name="blackbox"), "tenant id", True)
        workspace = output_field(self.task("create_workspace", tenant_id=tenant, name="blackbox"),
                                 "workspace id", True)
        key_id, key = self.issue(workspace, "schema")
        self.workspace, self.key_id, self.key = workspace, key_id, key
        self.server_files = [self.logs.enter_context((self.directory / name).open("wb"))
                             for name in ("server.stdout", "server.stderr")]
        self.server = subprocess.Popen(
            [str(self.args.binary), "--environment", "test_sqlite", "start", "--no-banner"],
            cwd=ROOT, env=self.env, stdout=self.server_files[0],
            stderr=self.server_files[1], start_new_session=True)
        deadline = time.monotonic() + self.remaining(45)
        while time.monotonic() < deadline:
            require(self.server.poll() is None, "server exited before readiness")
            try:
                status, _, _ = self.http("GET", "/api/schemas", key=key, timeout=1)
                if status == 200:
                    self.checks.append("authenticated server readiness")
                    return
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.1)
        raise AssertionError("server readiness deadline exceeded")

    def http(self, method, path, body=None, key=None, headers=None, rpc_id=None, timeout=10):
        self.remaining()
        connection = http.client.HTTPConnection("127.0.0.1", self.port,
                                                timeout=self.remaining(timeout))
        request_headers = {"Accept": "application/json", "Host": f"localhost:{self.port}"}
        if key is not None:
            request_headers["Authorization"] = f"Bearer {key}"
        if body is not None:
            request_headers["Content-Type"] = "application/json"
        request_headers.update(headers or {})
        raw = b""
        status = None
        try:
            connection.request(method, path, json.dumps(body) if body is not None else None,
                               request_headers)
            response = connection.getresponse()
            status = response.status
            response_headers = dict(response.getheaders())
            if "text/event-stream" in response.getheader("Content-Type", ""):
                # Stop after the matching event rather than waiting for an SSE connection to close.
                until = time.monotonic() + self.remaining(timeout)
                event = b""
                while time.monotonic() < until and len(raw) <= MAX_BODY:
                    if connection.sock is not None:
                        connection.sock.settimeout(max(0.001, until - time.monotonic()))
                    line = response.readline(MAX_BODY + 1)
                    if not line:
                        break
                    raw += line
                    event += line
                    if line in (b"\n", b"\r\n"):
                        try:
                            rpc_document(event.decode(), rpc_id)
                            break
                        except AssertionError:
                            event = b""
                else:
                    raise AssertionError("SSE deadline or size limit exceeded")
            else:
                raw = response.read(MAX_BODY + 1)
            require(len(raw) <= MAX_BODY, "HTTP body size limit exceeded")
            return status, response_headers, raw.decode("utf-8")
        finally:
            connection.close()
            self.events.append({"http": [method, path], "request": body, "status": status,
                                "response": raw.decode("utf-8", "replace")})

    def rest(self, method, path, body=None, status=200, key=None):
        actual, _, text = self.http(method, path, body, self.key if key is None else key)
        require(actual == status, f"{method} {path}: expected {status}, got {actual}: {text}")
        return json.loads(text) if text else None

    @staticmethod
    def stop(process):
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
        else:
            process.wait()

    def close(self):
        if self.server is not None:
            self.stop(self.server)
        self.logs.close()

    def artifacts(self, failure):
        destination = self.args.artifacts / f"{self.args.edition}-{self.args.suite}"
        destination.mkdir(parents=True, exist_ok=True)
        def write(name, text):
            # Replace rather than follow any existing artifact symlink.
            path = destination / name
            if path.is_symlink():
                raise ValueError(f"artifact symlink refused: {path}")
            path.write_text(redact(text, self.secrets), encoding="utf-8")
        write("transcript.json", json.dumps(self.events, indent=2))
        for name in ("server.stdout", "server.stderr"):
            path = self.directory / name
            write(name + ".log", path.read_text(errors="replace") if path.exists() else "")
        write("failure.log", failure or "")
        write("result.json", json.dumps({"suite": self.args.suite, "edition": self.args.edition,
              "passed": failure is None, "checks": self.checks,
              "duration_seconds": round(time.monotonic() - self.started, 2)}, indent=2))
        digest = hashlib.sha256()
        with self.args.binary.open("rb") as binary:
            for chunk in iter(lambda: binary.read(1024 * 1024), b""):
                digest.update(chunk)
        report = {"suite": self.args.suite, "edition": self.args.edition,
                  "binary_sha256": digest.hexdigest(), "results": self.results}
        report_path = self.args.artifacts / "results.json"
        require(not report_path.is_symlink(), "report symlink refused")
        report_path.write_text(json.dumps(report, indent=2), encoding="utf-8")


class MCP:
    def __init__(self, app, key):
        self.app, self.key = app, key
        self.session = None
        self.version = PROTOCOL
        result = self.rpc("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                          "clientInfo": {"name": "blackbox", "version": "1"}})["result"]
        require(self.session, "MCP initialize did not issue a session")
        require(result.get("capabilities", {}).get("tools") is not None, "MCP tools unsupported")
        self.version = result["protocolVersion"]
        self.rpc("notifications/initialized", notification=True)

    def rpc(self, method, params=None, notification=False, key=None):
        self.app.sequence += 1
        request_id = self.app.sequence
        body = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            body["params"] = params
        if not notification:
            body["id"] = request_id
        headers = {"Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": self.version}
        if self.session:
            headers["Mcp-Session-Id"] = self.session
        status, received, text = self.app.http("POST", "/mcp", body,
                                              self.key if key is None else key,
                                              headers, request_id)
        if method == "initialize":
            self.session = next((value for name, value in received.items()
                                 if name.lower() == "mcp-session-id"), None)
        require(status == (202 if notification else 200), f"MCP HTTP {status}: {text}")
        return None if notification else rpc_document(text, request_id)

    def call(self, name, arguments=None, error=False, key=None):
        document = self.rpc("tools/call", {"name": name, "arguments": arguments or {}}, key=key)
        if error:
            require(tool_error(document), f"expected MCP rejection: {name}: {document}")
            return document
        return tool_value(document)


def vertical_slice(app):
    app.rest("GET", "/api/schemas")
    client = MCP(app, app.key)
    require(isinstance(client.call("list_schemas"), list), "MCP list_schemas is not a list")
    app.task("revoke_api_key", key_id=app.key_id)
    app.rest("GET", "/api/schemas", status=401)
    document = client.call("list_schemas", error=True)
    require("error" in document and document["error"]["code"] == -32600,
            "revoked MCP credential must produce an authentication protocol error")
    _, app.key = app.issue(app.workspace, "schema")
    app.checks.append("CLI key issuance -> REST/MCP authentication -> CLI revoke -> both reject")


def crud(app, surface):
    client = MCP(app, app.key)
    # The workspace keeps its first schema binding, so retire a later unbound schema.
    app.rest("POST", "/api/schemas", {**SCHEMA, "name": "workspace-baseline"}, status=201)
    if surface == "rest":
        created = app.rest("POST", "/api/schemas", SCHEMA, status=201)
    else:
        created = client.call("create_schema", {"definition": SCHEMA})
    schema_id = created["schema"]["id"]
    require(app.rest("GET", f"/api/schemas/{schema_id}")["definition"]["name"] == "blackbox",
            "schema not visible through REST")
    require(client.call("get_active_schema", {"name": "blackbox"})["id"] == schema_id,
            "schema not visible through MCP")
    payload = {"schema_name": "blackbox", "entity_type": "note", "data": {"title": "before"}}
    if surface == "rest":
        entity = app.rest("POST", "/api/entities", payload, status=201)
    else:
        entity = client.call("create_entity", payload)
    entity_id = entity["id"]
    path = f"/api/entities/{entity_id}"
    require(app.rest("GET", path)["data"]["title"] == "before", "REST read-after-write failed")
    require(client.call("get_entity", {"id": entity_id})["data"]["title"] == "before",
            "MCP read-after-write failed")
    replacement = {"data": {"title": "after"}, "expected_updated_at": entity["updated_at"]}
    if surface == "rest":
        app.rest("PUT", path, replacement)
    else:
        client.call("update_entity", {"id": entity_id, **replacement})
    require(app.rest("GET", path)["data"]["title"] == "after", "REST update not visible")
    require(client.call("get_entity", {"id": entity_id})["data"]["title"] == "after",
            "MCP update not visible")
    require(any(item["id"] == entity_id for item in app.rest("GET", "/api/entities")),
            "REST entity list missing write")
    require(any(item["id"] == entity_id for item in client.call("list_entities")),
            "MCP entity list missing write")
    if surface == "rest":
        app.rest("DELETE", path, status=204)
        app.rest("POST", f"/api/schemas/{schema_id}/archive")
        app.rest("DELETE", f"/api/schemas/{schema_id}", status=204)
    else:
        client.call("delete_entity", {"id": entity_id})
        client.call("archive_schema", {"schema_id": schema_id})
        client.call("delete_schema", {"schema_id": schema_id})
    app.rest("GET", path, status=404)
    client.call("get_entity", {"id": entity_id}, error=True)
    app.rest("GET", f"/api/schemas/{schema_id}", status=404)
    client.call("get_schema_by_id", {"schema_id": schema_id}, error=True)
    app.checks.append(f"{surface} schema create/read/archive/delete and entity CRUD; cross-surface reads")


def rest_suite(app):
    _, read_key = app.issue(app.workspace, "read")
    app.rest("POST", "/api/schemas", SCHEMA, status=403, key=read_key)
    status, _, _ = app.http("GET", "/api/schemas")
    require(status == 401, "REST missing auth accepted")
    app.rest("GET", "/api/schemas", status=401, key="invalid")
    status, _, text = app.http("POST", "/api/entities", {"data": {}}, key=app.key)
    require(status == 422 and "schema_name" in text,
            f"wrong REST extractor rejection: {status}: {text}")
    app.checks.append("REST missing/invalid auth, read-only scope and malformed input")


def mcp_suite(app):
    client = MCP(app, app.key)
    listed = client.rpc("tools/list")["result"]
    names = {tool["name"] for tool in listed["tools"]}
    require({"list_schemas", "create_schema", "get_entity", "create_entity", "delete_entity"} <= names,
            "MCP discovery missing common tools")
    require(len(names) == len(listed["tools"]), "MCP discovery has duplicate names")
    require(not listed.get("nextCursor"), "MCP tool inventory unexpectedly paginated")
    for key in ("", "invalid"):
        document = client.call("list_schemas", error=True, key=key)
        require(document.get("error", {}).get("code") == -32600, "wrong MCP auth error")
    _, read_key = app.issue(app.workspace, "read")
    reader = MCP(app, read_key)
    denied = reader.call("create_schema", {"definition": SCHEMA}, error=True)
    require(denied.get("result", {}).get("isError") is True, "scope denial must be a tool error")
    malformed = client.call("get_entity", {"id": "not-a-uuid"}, error=True)
    require(malformed.get("result", {}).get("isError") is True
            and "failed to deserialize parameters" in json.dumps(malformed),
            "wrong MCP argument-deserialization tool error")
    client.call("create_schema", error=True)
    initialize = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}
    for headers, expected in (({"Host": "untrusted.invalid", "Accept": "application/json, text/event-stream"}, 403),
                              ({"Origin": "https://untrusted.invalid", "Accept": "application/json, text/event-stream"}, 403),
                              ({"Accept": "application/json"}, 406)):
        status, _, _ = app.http("POST", "/mcp", initialize, headers=headers)
        require(status == expected, f"MCP transport guard expected {expected}, got {status}")
    app.checks.append("MCP initialize/session/initialized/tools list; auth, scope, input errors")


def cli_suite(app):
    out, _ = app.cli("task")
    for name in ("create_tenant", "create_workspace", "create_api_key", "list_api_keys", "revoke_api_key"):
        require(re.search(r"^" + name + r"\s+\[", out, re.MULTILINE), f"task not registered: {name}")
    for arguments in (("--not-a-real-option",), ("task", "not_a_registered_task"),
                      ("task", "create_api_key"),
                      ("task", "create_api_key", "workspace_id:not-uuid", "scope:read"),
                      ("task", "create_api_key", f"workspace_id:{app.workspace}", "scope:invalid"),
                      ("task", "create_api_key", "malformed-argument")):
        out, err = app.cli(*arguments, ok=False)
        require(out.strip() or err.strip(), "CLI failure produced no diagnostic")
        require("api key (shown once):" not in out, "failed CLI issued a credential")
    key_id, key = app.issue(app.workspace, "read")
    listing = app.task("list_api_keys", workspace_id=app.workspace)
    require(key_id in listing and key not in listing, "CLI listing missing id or leaking credential")
    app.rest("GET", "/api/schemas", key=key)
    app.task("revoke_api_key", key_id=key_id)
    history = app.task("list_api_keys", workspace_id=app.workspace)
    require(key_id in history and key not in history, "CLI listing lost grant history or leaked credential")
    app.rest("GET", "/api/schemas", key=key, status=401)
    app.checks.append("CLI registered tasks, argument failures, exits/diagnostics, stdout IDs/key, listing/revoke")


def scenarios(suite):
    common = [("server.bootstrap", lambda app: app.boot()),
              ("auth.cli-issue-rest-mcp-revoke", vertical_slice)]
    focused = {
        "rest": [("rest.schema-entity-crud", lambda app: crud(app, "rest")),
                 ("rest.auth-scope-input", rest_suite)],
        "mcp": [("mcp.schema-entity-crud", lambda app: crud(app, "mcp")),
                ("mcp.discovery-auth-scope-input", mcp_suite)],
        "cli": [("cli.arguments-tasks-stdout-effects", cli_suite)],
    }
    return common + focused[suite]


def self_check():
    for suite in ("rest", "mcp", "cli"):
        definitions = scenarios(suite)
        ids = [name for name, _ in definitions]
        require(ids and len(ids) == len(set(ids)), "empty or duplicate scenario IDs")
        require(all(re.fullmatch(r"[a-z][a-z0-9.-]+", name) for name in ids), "unstable scenario ID")
        require(all(callable(function) for _, function in definitions), "invalid scenario function")
    require(SCHEMA["entity_types"]["note"]["fields"]["title"]["required"], "invalid fixture")


def main(argv=None):
    args = parse_args(argv)
    self_check()
    if args.self_check:
        print("blackbox scenario definitions: OK")
        return 0
    if args.list:
        print(json.dumps([name for name, _ in scenarios(args.suite)]))
        return 0
    def interrupted(signum, _frame):
        raise InterruptedError(f"runner interrupted by signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    failure = None
    # Owned state is never placed in the artifact directory or a caller's configured database.
    with tempfile.TemporaryDirectory(prefix="yorishiro-blackbox-") as directory:
        app = App(args, directory)
        try:
            for result, (_, function) in zip(app.results, scenarios(args.suite)):
                started = time.monotonic()
                try:
                    function(app)
                except Exception:
                    result["detail"] = "scenario failed; see redacted failure.log"
                    raise
                else:
                    result["status"] = "passed"
                    result.pop("detail")
                finally:
                    result["duration_seconds"] = round(time.monotonic() - started, 6)
        except (Exception, KeyboardInterrupt):  # noqa: BLE001
            failure = traceback.format_exc()
        finally:
            try:
                app.close()
            except Exception:  # noqa: BLE001
                failure = (failure or "") + traceback.format_exc()
                app.results[-1]["status"] = "failed"
                app.results[-1]["detail"] = "cleanup failed; see redacted failure.log"
            app.artifacts(failure)
    print(f"{args.edition}/{args.suite}: {'FAIL' if failure else 'PASS'}; artifacts: {args.artifacts}")
    return 1 if failure else 0


if __name__ == "__main__":
    sys.exit(main())
