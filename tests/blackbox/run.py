#!/usr/bin/env python3
"""External, dependency-free checks against an already-built Yorishiro binary."""

import argparse
import contextlib
import hashlib
import http.client
import http.server
import json
import os
import re
import signal
import socket
import subprocess
import sys
import tempfile
import threading
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
    text = re.sub(r"(invite token \(shown once\):\s*)[^\s\\]+", r"\1[REDACTED]", text)
    text = re.sub(r"(?i)(Bearer\s+)[^\s\"'\\,}]+", r"\1[REDACTED]", text)
    return text


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


def tool_text(document):
    require("error" not in document, f"RPC error: {document}")
    result = document["result"]
    require(not result.get("isError", False), f"tool error: {result}")
    texts = [item["text"] for item in result.get("content", []) if item["type"] == "text"]
    require(len(texts) == 1, "expected one tool text block")
    return texts[0]


def tool_value(document):
    return json.loads(tool_text(document))


def tool_error(document):
    return "error" in document or document.get("result", {}).get("isError") is True


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--suite", choices=("rest", "mcp", "cli", "tenant-postgres"))
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
        self.tool_calls = []
        self.discovered_tools = []
        self.server = None
        self.worker = None
        self.server_files = []
        self.logs = contextlib.ExitStack()
        self.sequence = 0
        self.started = time.monotonic()
        self.deadline = self.started + 240
        self.results = [{"id": name, "status": "failed", "duration_seconds": 0,
                         "detail": "not executed"} for name, _ in scenarios(args.suite, args.edition)]
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        # Only these ambient values are needed to execute a prebuilt binary.
        # Instrumented GHA binaries need their profile destination; application settings remain isolated.
        self.env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT", "LLVM_PROFILE_FILE") if key in os.environ}
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
            "YORISHIRO_MAX_TENANTS": "1",
            "YORISHIRO_AUTH_RATE_LIMIT_MAX": "10000",
            "YORISHIRO_MCP_ALLOWED_HOSTS": "localhost,127.0.0.1",
        })
        self.environment = "test_sqlite"
        if args.suite == "tenant-postgres":
            require(args.edition == "ee", "cross-tenant contracts require EE")
            database = os.environ.get("YORISHIRO_PUBLIC_POSTGRES_URL", "")
            licence = os.environ.get("YORISHIRO_LICENSE_KEY", "")
            require(database.startswith("postgres://") and licence,
                    "owned PostgreSQL URL and signed EE licence are required")
            self.environment = "test_postgres"
            self.secrets.extend([database, licence])
            self.env.update(DATABASE_URL=database, QUEUE_URL=database, LOCO_ENV=self.environment,
                            YORISHIRO_LICENSE_KEY=licence, YORISHIRO_MAX_TENANTS="0")
        config = self.directory / "config"
        config.mkdir()
        self.env["LOCO_CONFIG_FOLDER"] = str(config)
        text = (ROOT / f"config/{self.environment}.yaml").read_text()
        require(text.count("auto_migrate: true") == 1, "shipped SQLite config changed")
        (config / f"{self.environment}.yaml").write_text(text.replace("auto_migrate: true", "auto_migrate: false"))

    def remaining(self, cap=TIMEOUT):
        remaining = self.deadline - time.monotonic()
        require(remaining > 0, "suite deadline exceeded")
        return min(cap, remaining)

    def cli(self, *arguments, ok=True, stdin=None):
        command = [str(self.args.binary), "--environment", self.environment, *arguments]
        # File-backed capture stays bounded by disk, not pipe buffers, and survives timeout cleanup.
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            process = subprocess.Popen(command, cwd=ROOT, env=self.env,
                                       stdin=subprocess.PIPE if stdin is not None else None,
                                       stdout=stdout, stderr=stderr, start_new_session=True)
            try:
                process.communicate(input=stdin.encode() if stdin is not None else None,
                                    timeout=self.remaining())
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

    def issue(self, workspace, scope, *, issuer=None, audit=False, expires_at=None):
        if issuer is not None and expires_at is None:
            from datetime import UTC, datetime, timedelta
            expires_at = (datetime.now(UTC) + timedelta(minutes=30)).isoformat()
        body = {"name": "blackbox-delegated", "scope": scope, "audit": audit}
        if expires_at is not None:
            body["expires_at"] = expires_at
        created = self.rest("POST", "/api/api-keys", body, status=201, key=issuer or self.tenant_key,
            headers={"X-Workspace-Id": workspace})
        key = created["full_key"]
        self.secrets.append(key)
        require(key.startswith("ysr_"), "unexpected issued credential format")
        return str(uuid.UUID(created["id"])), key

    def create_workspace(self, name, *, issuer=None):
        return self.rest("POST", "/api/workspaces", {"name": name}, status=201,
                         key=issuer or self.tenant_key)["id"]

    def login(self, email, password):
        self.secrets.append(password)
        account = self.rest("POST", "/auth/root/login", {
            "email": email, "password": password,
        }, key="")
        self.secrets.append(account["api_key"])
        return account["api_key"]

    def boot(self):
        self.cli("db", "migrate")
        self.key = ""
        self.server_files = [self.logs.enter_context((self.directory / name).open("wb"))
                             for name in ("server.stdout", "server.stderr")]
        self.server = subprocess.Popen(
            [str(self.args.binary), "--environment", self.environment, "start", "--no-banner"],
            cwd=ROOT, env=self.env, stdout=self.server_files[0],
            stderr=self.server_files[1], start_new_session=True)
        deadline = time.monotonic() + self.remaining(45)
        while time.monotonic() < deadline:
            require(self.server.poll() is None, "server exited before readiness")
            try:
                status, _, _ = self.http("GET", "/setup/status", timeout=1)
                if status == 200:
                    break
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.1)
        else:
            raise AssertionError("server readiness deadline exceeded")
        password = "blackbox-owned-password"
        self.secrets.append(password)
        if self.args.suite == "tenant-postgres":
            account = self.rest("POST", "/auth/signup", {
                "email": "owner@example.invalid", "password": password,
            }, status=201, key="")
            self.tenant_key = self.login("owner@example.invalid", password)
            self.workspace = self.create_workspace("blackbox")
        else:
            account = self.rest("POST", "/setup", {
                "email": "owner@example.invalid", "password": password,
            }, status=201, key="")
            self.tenant_key = account["api_key"]
            self.secrets.append(self.tenant_key)
            self.workspace = account["workspace_id"]
        self.account = account
        self.tenant = account["tenant_id"]
        self.key_id, self.key = self.issue(self.workspace, "schema")

    def http(self, method, path, body=None, key=None, headers=None, rpc_id=None, timeout=10,
             raw_body=None):
        require(body is None or raw_body is None, "JSON and raw request bodies are exclusive")
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
        until = time.monotonic() + self.remaining(timeout)
        try:
            connection.request(method, path, raw_body if raw_body is not None else
                               json.dumps(body) if body is not None else None,
                               request_headers)
            response_socket = connection.sock
            response = connection.getresponse()
            status = response.status
            response_headers = dict(response.getheaders())
            sse = "text/event-stream" in response.getheader("Content-Type", "")
            pending, event = b"", b""
            matched = False
            while not matched:
                remaining = min(self.remaining(timeout), until - time.monotonic())
                require(remaining > 0, "HTTP response deadline exceeded")
                if response_socket is not None:
                    response_socket.settimeout(remaining)
                # read1 returns available bytes, so a trickled partial line cannot reset the deadline.
                chunk = response.read1(min(65536, MAX_BODY + 1 - len(raw)))
                self.remaining()
                require(time.monotonic() < until, "HTTP response deadline exceeded")
                if not chunk:
                    break
                raw += chunk
                require(len(raw) <= MAX_BODY, "HTTP body size limit exceeded")
                if sse:
                    pending += chunk
                    while b"\n" in pending:
                        line, _, pending = pending.partition(b"\n")
                        event += line + b"\n"
                        if line in (b"", b"\r"):
                            try:
                                rpc_document(event.decode(), rpc_id)
                                matched = True
                                raw = raw[:len(raw) - len(pending)]
                                break
                            except AssertionError:
                                event = b""
            return status, response_headers, raw.decode("utf-8")
        finally:
            connection.close()
            self.events.append({"http": [method, path], "request": raw_body if raw_body is not None else body, "status": status,
                                "response": raw.decode("utf-8", "replace")})

    def rest(self, method, path, body=None, status=200, key=None, headers=None):
        actual, _, text = self.http(method, path, body, self.key if key is None else key,
                                    headers=headers)
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
        try:
            if self.worker is not None:
                self.stop(self.worker)
        finally:
            try:
                if self.server is not None:
                    self.stop(self.server)
            finally:
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
        for name in ("server.stdout", "server.stderr", "worker.stdout", "worker.stderr"):
            path = self.directory / name
            write(name + ".log", path.read_text(errors="replace") if path.exists() else "")
        write("failure.log", failure or "")
        digest = hashlib.sha256()
        with self.args.binary.open("rb") as binary:
            for chunk in iter(lambda: binary.read(1024 * 1024), b""):
                digest.update(chunk)
        report = {"suite": self.args.suite, "edition": self.args.edition,
                  "binary_sha256": digest.hexdigest(), "results": self.results,
                  "discovered_tools": self.discovered_tools, "tool_calls": self.tool_calls}
        report_path = self.args.artifacts / "results.json"
        require(not report_path.is_symlink(), "report symlink refused")
        report_path.write_text(json.dumps(report, indent=2), encoding="utf-8")

    @contextlib.contextmanager
    def job_worker(self, target="job-target:shared"):
        require(self.worker is None, "worker already started")
        previous_deadline = self.deadline
        self.deadline = min(previous_deadline, time.monotonic() + 60)
        try:
            files = [self.logs.enter_context((self.directory / name).open("wb"))
                     for name in ("worker.stdout", "worker.stderr")]
            self.worker = subprocess.Popen(
                [str(self.args.binary), "--environment", self.environment, "start",
                 f"--worker={target}", "--no-banner"],
                cwd=ROOT, env=self.env, stdout=files[0], stderr=files[1], start_new_session=True)
            yield self.worker
        finally:
            self.deadline = previous_deadline
            if self.worker is not None:
                self.stop(self.worker)
                self.worker = None


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

    def call(self, name, arguments=None, error=False, key=None, raw=False):
        document = self.rpc("tools/call", {"name": name, "arguments": arguments or {}}, key=key)
        if error:
            require(tool_error(document), f"expected MCP rejection: {name}: {document}")
            self.app.tool_calls.append({"tool": name, "scenario": self.app.current_scenario, "outcome": "rejected"})
            return document
        value = tool_text(document) if raw else tool_value(document)
        self.app.tool_calls.append({"tool": name, "scenario": self.app.current_scenario, "outcome": "succeeded"})
        return value


def vertical_slice(app):
    app.rest("GET", "/api/schemas")
    client = MCP(app, app.key)
    require(isinstance(client.call("list_schemas"), list), "MCP list_schemas is not a list")
    app.rest("DELETE", f"/api/api-keys/{app.key_id}", status=204, key=app.tenant_key)
    app.rest("GET", "/api/schemas", status=401)
    document = client.call("list_schemas", error=True)
    require("error" in document and document["error"]["code"] == -32600,
            "revoked MCP credential must produce an authentication protocol error")
    app.key_id, app.key = app.issue(app.workspace, "schema")


def crud(app, surface):
    client = MCP(app, app.key)
    # The workspace keeps its first schema binding, so retire a later unbound schema.
    app.rest("POST", "/api/schemas", {**SCHEMA, "name": "workspace-baseline"}, status=201)
    if surface == "rest":
        created = app.rest("POST", "/api/schemas", SCHEMA, status=201)
    else:
        created = client.call("create_schema", {"definition": SCHEMA})
    schema_id = created["schema"]["id"]
    historical_definition = app.rest("GET", f"/api/schemas/{schema_id}")["definition"]
    require(historical_definition["name"] == "blackbox",
            "schema not visible through REST")
    require(client.call("get_active_schema", {"name": "blackbox"})["id"] == schema_id,
            "schema not visible through MCP")
    app.rest("DELETE", f"/api/schemas/{schema_id}", status=409)
    evolved = {**SCHEMA, "entity_types": {"note": {"fields": {
        **SCHEMA["entity_types"]["note"]["fields"], "extra": {"type": "string"}}}}}
    next_version = app.rest("POST", "/api/schemas", evolved, status=201)["schema"]
    require(next_version["version"] == created["schema"]["version"] + 1,
            "schema evolution did not increment version")
    require(client.call("get_active_schema", {"name": "blackbox"})["id"] == next_version["id"],
            "schema evolution did not change active version")
    require(app.rest("GET", f"/api/schemas/{schema_id}")["definition"] == historical_definition,
            "schema evolution mutated historical definition")
    schema_id = next_version["id"]
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
    app.rest("PUT", path, replacement, status=409)
    client.call("update_entity", {"id": entity_id, **replacement}, error=True)
    require(app.rest("GET", path)["data"]["title"] == "after", "stale update changed data")
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


class GraphAPI:
    """Only the common operations used by the relation/import scenario."""

    def __init__(self, app, surface, key):
        self.app, self.surface, self.key = app, surface, key
        self.client = MCP(app, key)

    def call(self, name, arguments=None):
        arguments = arguments or {}
        if self.surface == "mcp":
            return self.client.call(name, arguments)
        routes = {
            "create_schema": ("POST", "/api/schemas", arguments.get("definition"), 201),
            "create_entity": ("POST", "/api/entities", arguments, 201),
            "create_relation": ("POST", "/api/relations", arguments, 201),
            "delete_relation": ("DELETE", f"/api/relations/{arguments.get('id')}", None, 204),
            "set_relation_status": ("PUT", f"/api/relations/{arguments.get('id')}/status",
                                    {"status": arguments.get("status")}, 200),
            "list_schemas": ("GET", "/api/schemas", None, 200),
            "list_entities": ("GET", "/api/entities", None, 200),
            "list_relations": ("GET", "/api/relations", None, 200),
            "import_job_status": ("GET", f"/api/import-jobs/{arguments.get('job_id')}", None, 200),
        }
        method, path, body, status = routes[name]
        return self.app.rest(method, path, body, status=status, key=self.key)

    def export(self):
        if self.surface == "mcp":
            return self.client.call("export_jsonl", raw=True)
        status, headers, text = self.app.http("GET", "/api/export.jsonl", key=self.key)
        require(status == 200, f"export failed: {status}: {text}")
        content_type = next((value for name, value in headers.items()
                             if name.lower() == "content-type"), "")
        require(content_type == "application/x-ndjson", "export is not raw NDJSON")
        return text

    def submit(self, text):
        if self.surface == "mcp":
            return self.client.call("import_jsonl", {"jsonl": text})
        status, _, body = self.app.http("POST", "/api/import.jsonl", key=self.key,
                                       headers={"Content-Type": "application/x-ndjson"}, raw_body=text)
        require(status == 202, f"import was not accepted: {status}: {body}")
        return json.loads(body)


def relation_export_import_worker(app, surface):
    def workspace(name):
        value = app.create_workspace(name)
        _, key = app.issue(value, "schema")
        return value, key

    source_id, source_key = workspace("graph-source")
    source = GraphAPI(app, surface, source_key)
    peer = GraphAPI(app, "mcp" if surface == "rest" else "rest", source_key)
    definition = {"name": "graph", "entity_types": {"person": {"fields": {
        "name": {"type": "string", "required": True}}}},
        "relation_types": {"knows": {"source": "person", "target": "person"}}}
    schema_id = source.call("create_schema", {"definition": definition})["schema"]["id"]
    _, write_key = app.issue(source_id, "write")
    source = GraphAPI(app, surface, write_key)
    peer = GraphAPI(app, peer.surface, write_key)
    people = [source.call("create_entity", {"schema_name": "graph", "entity_type": "person",
                                         "data": {"name": name}}) for name in ("Alice", "Bob")]
    payload = {"source_id": people[0]["id"], "target_id": people[1]["id"], "relation_type": "knows"}
    _, read_key = app.issue(source_id, "read")
    readers = (GraphAPI(app, "rest", read_key), GraphAPI(app, "mcp", read_key))
    require(len(readers[1].client.call("list_entities", {"entity_type": "person"})) == 2,
            "filtered MCP entity list changed")
    invalid_list = readers[1].client.call("list_relations", {"status": "not-a-status"}, error=True)
    require(invalid_list.get("result", {}).get("isError") is True
            and "relation status" in invalid_list["result"]["content"][0]["text"],
            "wrong invalid relation-list status error")
    missing_entity = readers[1].client.call("get_entity", {"id": str(uuid.uuid4())}, error=True)
    require(missing_entity.get("result", {}).get("isError") is True
            and "not found" in missing_entity["result"]["content"][0]["text"], "wrong missing entity error")

    def read_relation(relation):
        rest = app.rest("GET", f"/api/relations/{relation['id']}", key=read_key)
        mcp = readers[1].client.call("get_relation", {"id": relation["id"]})
        require(rest == mcp == relation, "relation readback differs across surfaces")
        for api in readers:
            require(api.call("list_relations") == [relation], "relation list differs across surfaces")
        filtered = app.rest("GET", f"/api/relations?source_id={people[0]['id']}", key=read_key)
        require(filtered == readers[1].call("list_relations", {"source_id": people[0]["id"]}) == [relation],
                "source-filtered relation list differs across surfaces")

    relation = source.call("create_relation", payload)
    require(relation["properties"] == {} and relation["status"] == "active",
            "omitted relation properties/status defaults changed")
    read_relation(relation)
    recall = readers[1].client.call("recall_context", {"entity_id": people[0]["id"], "full": True})
    require(recall["entity"] == people[0] and len(recall["relations"]) == 1
            and recall["relations"][0]["neighbor"] == people[1]
            and recall["relations"][0]["hop_distance"] == 1
            and recall["relations"][0]["relation_type"] == "knows" and not recall["truncated"],
            "recall did not return the connected full neighbor")
    shallow = readers[1].client.call("recall_context", {"entity_id": people[0]["id"]})
    require(shallow["entity"] == people[0] and shallow["relations"][0]["neighbor"]["data"] == {},
            "default recall exposed non-embedding neighbor fields")
    readers[1].client.call("recall_context", {"entity_id": str(uuid.uuid4())}, error=True)
    _, isolated_key = app.issue(app.workspace, "read")
    MCP(app, isolated_key).call("recall_context", {"entity_id": people[0]["id"]}, error=True)
    app.rest("POST", "/api/relations", payload, status=403, key=read_key)
    denied = MCP(app, read_key).call("create_relation", payload, error=True)
    require(denied.get("result", {}).get("isError") is True
            and "scope_insufficient" in json.dumps(denied), "wrong relation scope tool error")
    status, _, _ = app.http("POST", "/api/relations", payload)
    require(status == 401, "relation creation accepted missing auth")
    status, _, _ = app.http("GET", "/api/relations")
    require(status == 401, "relation listing accepted missing auth")
    unauthenticated = source.client.call("create_relation", payload, error=True, key="")
    require(unauthenticated.get("error", {}).get("code") == -32600, "wrong MCP missing-auth error")
    status, _, text = app.http("PUT", f"/api/relations/{relation['id']}/status",
                               {"status": "invalid"}, key=source_key)
    require(status == 422 and "invalid" in text, "wrong invalid relation status rejection")
    invalid = source.client.call("set_relation_status", {"id": relation["id"], "status": "invalid"}, error=True)
    require(invalid.get("result", {}).get("isError") is True
            and "not a relation status" in json.dumps(invalid), "wrong MCP relation status error")
    missing = {**payload, "target_id": str(uuid.uuid4())}
    app.rest("POST", "/api/relations", missing, status=404, key=source_key)
    rejected = source.client.call("create_relation", missing, error=True)
    require(rejected.get("result", {}).get("isError") is True
            and "not found" in json.dumps(rejected), "wrong MCP missing-entity error")
    for state in ("deprecated", "archived", "active"):
        relation = source.call("set_relation_status", {"id": relation["id"], "status": state})
        require(relation["status"] == state, "relation status update ignored")
        read_relation(relation)
    source.call("delete_relation", {"id": relation["id"]})
    app.rest("GET", f"/api/relations/{relation['id']}", status=404, key=source_key)
    source.client.call("get_relation", {"id": relation["id"]}, error=True)
    require(source.call("list_relations") == peer.call("list_relations") == [], "deleted relation still listed")
    require(readers[1].client.call("recall_context", {"entity_id": people[0]["id"]})["relations"] == [],
            "recall retained a deleted relation")
    relation = source.call("create_relation", {**payload, "properties": {"since": 2026}})
    relation = source.call("set_relation_status", {"id": relation["id"], "status": "deprecated"})
    require(relation["properties"] == {"since": 2026} and relation["status"] == "deprecated",
            "relation properties or deprecated status lost")
    read_relation(relation)
    exported = readers[0].export()
    require(exported == readers[1].export(), "REST/MCP raw export text differs")
    records = [json.loads(line) for line in exported.splitlines()]
    require([item["kind"] for item in records] == ["schema", "entity", "entity", "relation"],
            "export record counts or kind order differs")
    schema = app.rest("GET", f"/api/schemas/{schema_id}", key=source_key)
    expected = [schema, *sorted(people, key=lambda person: person["id"]), relation]
    require([item["record"] for item in records] == expected, "export changed IDs/data/properties/status")

    destination_id, destination_key = workspace("graph-destination")
    destination = GraphAPI(app, surface, destination_key)
    destination_peer = GraphAPI(app, peer.surface, destination_key)
    job_id = destination.submit(exported)["job_id"]
    require(str(uuid.UUID(job_id)) == job_id, "import returned noncanonical job ID")
    for api in (destination, destination_peer):
        job = api.call("import_job_status", {"job_id": job_id})
        require(job == {"job_id": job_id, "job_name": "import", "status": "queued", "result": None},
                "import was not queued before worker startup")
        for name in ("list_schemas", "list_entities", "list_relations"):
            require(api.call(name) == [], "import mutated destination before worker startup")
    with app.job_worker() as worker:
        while True:
            app.remaining()
            require(worker.poll() is None, "import worker exited before completion")
            require(app.server.poll() is None, "server exited during import")
            job = destination.call("import_job_status", {"job_id": job_id})
            app.remaining()
            require(job["job_id"] == job_id and job["job_name"] == "import", "wrong import job identity")
            require(job["status"] in ("queued", "running", "completed"), f"import failed: {job}")
            if job["status"] == "completed":
                require(worker.poll() is None, "import worker exited at completion")
                break
            time.sleep(min(0.1, app.remaining()))
        require(destination_peer.call("import_job_status", {"job_id": job_id}) == job,
                "completed import status differs across surfaces")
        require(job["result"] == {"job_id": job_id, "schemas": 1, "entities": 2, "relations": 1},
                "wrong completed import counts")

    restored = {}
    for name, count in (("list_schemas", 1), ("list_entities", 2), ("list_relations", 1)):
        rows = destination.call(name)
        require(rows == destination_peer.call(name) and len(rows) == count,
                f"restored {name} differs across surfaces or has wrong count")
        if name != "list_schemas":
            require(all(row["workspace_id"] == destination_id for row in rows), "wrong destination ownership")
        restored[name] = rows
    new_schema_id = restored["list_schemas"][0]["id"]
    new_schema = app.rest("GET", f"/api/schemas/{new_schema_id}", key=destination_key)
    require(new_schema == destination.client.call("get_schema_by_id", {"schema_id": new_schema_id}),
            "restored schema readback differs")
    require(new_schema["id"] != schema_id and new_schema["workspace_id"] == destination_id
            and new_schema["tenant_id"] == app.tenant
            and new_schema["definition"] == schema["definition"] and new_schema["status"] == "active",
            "import did not recreate the schema")
    new_people = {row["data"]["name"]: row for row in restored["list_entities"]}
    require(set(new_people) == {"Alice", "Bob"}, "import changed entity data")
    old_ids = {row["id"] for row in people}
    for name, row in new_people.items():
        require(row["id"] not in old_ids and row["schema_id"] == new_schema["id"]
                and row["schema_version"] == new_schema["version"] and row["entity_type"] == "person"
                and row["data"] == {"name": name}, "import did not remap entity IDs/schema/data")
        require(app.rest("GET", f"/api/entities/{row['id']}", key=destination_key)
                == destination.client.call("get_entity", {"id": row["id"]}) == row,
                "restored entity readback differs")
    new_relation = restored["list_relations"][0]
    require(new_relation["id"] != relation["id"]
            and new_relation["source_id"] == new_people["Alice"]["id"]
            and new_relation["target_id"] == new_people["Bob"]["id"]
            and new_relation["relation_type"] == "knows" and new_relation["properties"] == {"since": 2026}
            and new_relation["status"] == "active", "import did not restore/remap the active relation")
    require(app.rest("GET", f"/api/relations/{new_relation['id']}", key=destination_key)
            == destination.client.call("get_relation", {"id": new_relation["id"]}) == new_relation,
            "restored relation readback differs")
    require(source.export() == peer.export() == exported, "import mutated the source graph")


def template_required_data(fields):
    values = {"integer": 1, "number": 1, "boolean": True, "array": [], "object": {}}
    formats = {"date": "2026-01-01", "date-time": "2026-01-01T00:00:00Z",
               "uri": "https://example.com", "email": "test@example.com"}
    result = {}
    for name, field in fields.items():
        if not field.get("required", False):
            continue
        if field["type"] != "string":
            result[name] = values[field["type"]]
        else:
            result[name] = formats.get(field.get("format"),
                                      "0000-0000-0000-000X" if field.get("pattern")
                                      else next(iter(field.get("enum") or []), "test"))
    return result


def builtin_relations(app):
    workspace = app.create_workspace("builtin-relations")
    _, schema_key = app.issue(workspace, "schema")
    _, write_key = app.issue(workspace, "write")
    templates = app.rest("GET", "/api/templates", key=schema_key)
    client = MCP(app, schema_key)
    require(client.call("list_templates") == templates, "builtin inventory differs across transports")
    require(templates, "builtin template inventory is empty")
    for template in templates:
        definition = app.rest("GET", f"/api/templates/{template['id']}", key=schema_key)
        require(client.call("get_template", {"template_id": template["id"]}) == definition,
                "builtin definition differs across transports")
        app.rest("POST", "/api/schemas", definition, status=201, key=schema_key)
        for entity_type in definition["entity_types"]:
            document = client.call("get_entity_type_json_schema", {
                "schema_name": definition["name"], "entity_type": entity_type,
            })
            require(document["type"] == "object" and "properties" in document,
                    "entity type did not expose an object JSON Schema")

        def create(entity_type, definition=definition):
            return app.rest("POST", "/api/entities", {
                "schema_name": definition["name"], "entity_type": entity_type,
                "data": template_required_data(definition["entity_types"][entity_type]["fields"]),
            }, status=201, key=write_key)["id"]

        entities = {name: create(name) for name in definition["entity_types"]}
        for name, relation in definition.get("relation_types", {}).items():
            target = create(relation["target"]) if relation["source"] == relation["target"] else entities[relation["target"]]
            app.rest("POST", "/api/relations", {
                "source_id": entities[relation["source"]], "target_id": target,
                "relation_type": name, "properties": {},
            }, status=201, key=write_key)


def rest_suite(app):
    _, read_key = app.issue(app.workspace, "read")
    app.rest("POST", "/api/schemas", SCHEMA, status=403, key=read_key)
    status, _, _ = app.http("GET", "/api/schemas")
    require(status == 401, "REST missing auth accepted")
    app.rest("GET", "/api/schemas", status=401, key="invalid")
    status, _, text = app.http("POST", "/api/entities", {"data": {}}, key=app.key)
    require(status == 422 and "schema_name" in text,
            f"wrong REST extractor rejection: {status}: {text}")


def mcp_suite(app):
    client = MCP(app, app.key)
    listed = client.rpc("tools/list")["result"]
    names = {tool["name"] for tool in listed["tools"]}
    reviewed = json.loads((ROOT / "tests/blackbox/tools.json").read_text())
    expected = set(reviewed["ce"])
    if app.args.edition == "ee":
        expected.update(reviewed["ee_additions"])
    require(names == expected, "MCP public-tool inventory changed; review scenario ownership and gaps")
    app.discovered_tools = sorted(names)
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


def cli_suite(app):
    out, _ = app.cli("task")
    registered = set(re.findall(r"^(\w+)\s+\[", out, re.MULTILINE))
    retained = {"inspect_tenant_retirement", "disable_tenant", "delete_tenant", "delete_user",
                "resync_embeddings", "embedding_index_maintenance", "maintenance", "maintenance_status",
                "recover_worker_dispatches", "tenant_reindex_scheduler", "db_load_guard",
                "reset_root_password"}
    removed = {"create_tenant", "create_user", "add_member", "create_invite", "list_members",
               "list_tenants", "list_users", "create_workspace", "list_workspaces", "create_api_key",
               "list_api_keys", "revoke_api_key", "create_tenant_api_key", "reindex_embeddings"}
    require(retained <= registered, f"retained tasks missing: {retained - registered}")
    require(not removed & registered, f"removed tasks still registered: {removed & registered}")
    for arguments in (("--not-a-real-option",), ("task", "not_a_registered_task"),
                      ("task", "maintenance"), ("task", "maintenance", "mode:invalid"),
                      ("task", "resync_embeddings", "workspace_id:not-uuid"),
                      ("task", "maintenance", "malformed-argument")):
        out, err = app.cli(*arguments, ok=False)
        require(out.strip() or err.strip(), "CLI failure produced no diagnostic")
    require("mode=off" in app.task("maintenance_status"), "initial maintenance state is not off")
    app.task("maintenance", mode="read_only", retry_after=7, reason="blackbox-maintenance")
    try:
        state = app.task("maintenance_status")
        require("mode=read_only" in state and "retry_after=7s" in state
                and "blackbox-maintenance" in state, "maintenance task did not persist its arguments")
        app.rest("POST", "/api/entities", {}, status=423)
        app.rest("GET", "/api/schemas")
    finally:
        app.task("maintenance", mode="off")
    require("mode=off" in app.task("maintenance_status"), "maintenance task did not restore service")


def root_recovery(app):
    password = "blackbox-recovered-password"
    app.secrets.append(password)
    user_id = app.account["user_id"]
    old_key = app.tenant_key
    app.cli("task", "reset_root_password", f"user_id:{user_id}",
            f"confirmed_user_id:{user_id}", stdin=password + "\n")
    app.rest("GET", "/api/whoami", key=old_key, status=401)
    key = app.login("owner@example.invalid", password)
    app.rest("GET", "/api/whoami", key=key)


def human_owner(app):
    _, owner = app.issue(app.workspace, "schema")
    client = MCP(app, owner)
    for identity in (app.rest("GET", "/api/whoami", key=owner), client.call("whoami")):
        require(identity["user_id"] == app.account["user_id"] and identity["identity_kind"] == "user",
                "delegated credential is not attributed to the bootstrap human")
    app.management_key = owner
    return app.account, owner


def iam_lifecycle(app):
    client = MCP(app, app.tenant_key)
    group = client.call("create_iam_group", {"name": "operators"})
    group_id = group["id"]
    require(client.call("get_iam_group", {"group_id": group_id}) == group,
            "IAM group lookup differs")
    require(group in client.call("list_iam_groups"), "IAM group missing from list")
    renamed = client.call("update_iam_group", {"group_id": group_id, "name": "reviewers"})
    require(renamed["name"] == "reviewers", "IAM group rename failed")
    password = "blackbox-iam-member-password"
    app.secrets.append(password)
    account = app.rest("POST", "/api/members/users", {
        "user_name": "iam-member", "password": password, "role": "admin",
    }, status=201, key=app.tenant_key)
    user_id = account["user_id"]
    document = {"statements": [{"effect": "allow", "actions": ["workspace_get"],
                               "resources": [{"type": "workspace", "tenant_id": app.tenant,
                                              "workspace_id": app.workspace}]}]}
    policy = client.call("create_iam_policy", {"name": "workspace-reader", "document": document})
    policy_id = policy["id"]
    require(client.call("get_iam_policy", {"policy_id": policy_id}) == policy,
            "IAM policy lookup differs")
    require(policy in client.call("list_iam_policies"), "IAM policy missing from list")
    membership = {"group_id": group_id, "user_id": user_id}
    group_policy = {"group_id": group_id, "policy_id": policy_id}
    user_policy = {"user_id": user_id, "policy_id": policy_id}
    client.call("add_group_member", membership)
    client.call("attach_group_policy", group_policy)
    client.call("attach_user_policy", user_policy)
    for tool, args, path in (
        ("list_iam_group_members", {"group_id": group_id}, f"groups/{group_id}/members"),
        ("list_iam_group_policies", {"group_id": group_id}, f"groups/{group_id}/policies"),
        ("list_iam_user_policies", {"user_id": user_id}, f"users/{user_id}/policies"),
    ):
        rows = client.call(tool, args)
        require(len(rows) == 1 and rows == app.rest("GET", f"/api/iam/{path}", key=app.tenant_key),
                f"IAM association REST/MCP mismatch: {tool}")
    member_name = app.rest("GET", "/api/members", key=app.tenant_key)
    member_name = next(row["user_name"] for row in member_name if row["user_id"] == user_id)
    login = app.rest("POST", "/auth/tenant/login", {"user_name": member_name, "password": password},
                     key="", headers={"X-Tenant-Id": app.tenant})
    member_key = login["api_key"]
    app.secrets.append(member_key)
    member_identity = app.rest("GET", "/api/whoami", key=member_key)
    member_capabilities = MCP(app, member_key).call("capabilities")
    require(member_identity == MCP(app, member_key).call("whoami"), "identity transport mismatch")
    require(not member_identity["tenant_root"] and not member_capabilities["tenant_root"],
            "legacy administrator role was reported as IAM root")
    require("workspace_access" not in member_identity and "tenant_admin" not in member_capabilities
            and "workspace_access" not in member_capabilities, "discovery retained legacy role authority")
    require(member_capabilities["credential_scope"] == member_identity["stored_scope"]
            and member_capabilities["constraints"]["authorization_model"] == "tenant_iam"
            and not member_capabilities["constraints"]["discovery_authorizes_requests"],
            "discovery confused credential ceiling with IAM authorization")
    # Root's independent audit grant is not inherited by an ordinary subject.
    no_audit = MCP(app, member_key)
    no_audit.call("list_audit_log", error=True)
    selected = {"X-Workspace-Id": app.workspace}
    document["statements"].append({"effect": "allow", "actions": ["credential_create"],
                                   "resources": [{"type": "tenant", "tenant_id": app.tenant}]})
    client.call("update_iam_policy", {"policy_id": policy_id, "name": "workspace-reader", "document": document})
    _, bound_key = app.issue(app.workspace, "schema", issuer=member_key)
    document["statements"].pop()
    client.call("update_iam_policy", {"policy_id": policy_id, "name": "workspace-reader", "document": document})
    ordinary = MCP(app, bound_key)
    for tool in ("list_entities", "list_schemas", "list_relations"):
        ordinary.call(tool, error=True)
    ordinary.call("search_entities", {"query_text": "denied"}, error=True)
    ordinary.call("export_jsonl", error=True)
    ordinary.call("recall_context", {"entity_id": str(uuid.uuid4())}, error=True)
    for path in ("/api/entities", "/api/schemas", "/api/relations", "/api/search?query_text=denied", "/api/export.jsonl"):
        app.rest("GET", path, status=403, key=bound_key)
    actions = ["workspace_get", "entity_create", "entity_get", "entity_list", "entity_update", "entity_delete",
               "schema_create", "schema_get", "schema_list", "schema_archive", "schema_delete",
               "relation_create", "relation_get", "relation_list", "relation_update", "relation_delete",
               "recall", "export", "audit_log_list"]
    document["statements"][0]["actions"] = actions
    client.call("update_iam_policy", {"policy_id": policy_id, "name": "workspace-reader", "document": document})
    ordinary.call("list_audit_log", error=True)
    app.rest("GET", "/api/audit-log", status=403, key=bound_key)
    definition = {**SCHEMA, "name": "iam-note",
                  "relation_types": {"links": {"source": "note", "target": "note"}}}
    schema = ordinary.call("create_schema", {"definition": definition})["schema"]
    require(ordinary.call("get_schema_by_id", {"schema_id": schema["id"]}) ==
            app.rest("GET", f"/api/schemas/{schema['id']}", key=bound_key), "IAM schema transport mismatch")
    entity = ordinary.call("create_entity", {"schema_name": "iam-note", "entity_type": "note",
                                            "data": {"title": "iam-original"}})
    entity_path = f"/api/entities/{entity['id']}"
    target = ordinary.call("create_entity", {"schema_name": "iam-note", "entity_type": "note",
                                            "data": {"title": "iam-target"}})
    relation = ordinary.call("create_relation", {"source_id": entity["id"], "target_id": target["id"],
                                                "relation_type": "links"})
    relation_path = f"/api/relations/{relation['id']}"
    ordinary.call("recall_context", {"entity_id": entity["id"]})
    ordinary.call("export_jsonl", raw=True)
    export_status, _, exported = app.http("GET", "/api/export.jsonl", key=bound_key)
    require(export_status == 200 and any(json.loads(line)["kind"] == "entity" for line in exported.splitlines()),
            "delegated REST export lost entity records")
    require(ordinary.call("get_relation", {"id": relation["id"]}) ==
            app.rest("GET", relation_path, key=bound_key), "IAM relation transport mismatch")
    require(ordinary.call("get_entity", {"id": entity["id"]}) == app.rest("GET", entity_path, key=bound_key),
            "IAM entity transport mismatch")
    document["statements"].append({"effect": "deny", "actions": ["entity_update", "schema_archive", "relation_delete", "recall", "export"],
                                   "resources": document["statements"][0]["resources"]})
    client.call("update_iam_policy", {"policy_id": policy_id, "name": "workspace-reader", "document": document})
    ordinary.call("update_entity", {"id": entity["id"], "data": {"title": "denied"}}, error=True)
    app.rest("PUT", entity_path, {"data": {"title": "denied"}}, status=403, key=bound_key)
    ordinary.call("archive_schema", {"schema_id": schema["id"]}, error=True)
    app.rest("POST", f"/api/schemas/{schema['id']}/archive", status=403, key=bound_key)
    ordinary.call("delete_relation", {"id": relation["id"]}, error=True)
    app.rest("DELETE", relation_path, status=403, key=bound_key)
    ordinary.call("recall_context", {"entity_id": entity["id"]}, error=True)
    ordinary.call("export_jsonl", error=True)
    app.rest("GET", "/api/export.jsonl", status=403, key=bound_key)
    require(app.rest("GET", relation_path, key=bound_key) == relation, "denied deletion changed relation")
    require(app.rest("GET", entity_path, key=bound_key)["data"] == entity["data"], "denied IAM write changed entity")
    document["statements"].pop()
    client.call("update_iam_policy", {"policy_id": policy_id, "name": "workspace-reader", "document": document})
    ordinary.call("update_entity", {"id": entity["id"], "data": {"title": "allowed"}})
    ordinary.call("delete_relation", {"id": relation["id"]})
    app.rest("DELETE", f"/api/entities/{target['id']}", status=204, key=bound_key)
    app.rest("DELETE", entity_path, status=204, key=bound_key)
    ordinary.call("archive_schema", {"schema_id": schema["id"]})
    app.rest("DELETE", f"/api/schemas/{schema['id']}", status=204, key=bound_key)
    app.rest("GET", f"/api/workspaces/{app.workspace}", key=member_key, headers=selected)
    client.call("detach_user_policy", user_policy)
    app.rest("GET", f"/api/workspaces/{app.workspace}", key=member_key, headers=selected)
    denied = {"statements": [{**document["statements"][0], "effect": "deny"}]}
    client.call("update_iam_policy", {"policy_id": policy_id, "name": "workspace-denied", "document": denied})
    app.rest("GET", f"/api/workspaces/{app.workspace}", status=403, key=member_key, headers=selected)
    client.call("detach_group_policy", group_policy)
    ordinary.call("list_entities", error=True)
    ordinary.call("list_schemas", error=True)
    app.rest("GET", "/api/entities", status=403, key=bound_key)
    app.rest("GET", "/api/schemas", status=403, key=bound_key)
    ordinary.call("list_workspace_members", {"workspace_id": app.workspace}, error=True)
    ordinary.call("grant_workspace_member", {"workspace_id": app.workspace, "user_id": user_id,
                                              "role": "viewer"}, error=True)
    client.call("remove_group_member", membership)
    require(not client.call("list_iam_group_members", {"group_id": group_id}), "IAM membership retained")
    require(not client.call("list_iam_group_policies", {"group_id": group_id}), "IAM group grant retained")
    require(not client.call("list_iam_user_policies", {"user_id": user_id}), "IAM user grant retained")
    client.call("delete_iam_policy", {"policy_id": policy_id})
    client.call("delete_iam_group", {"group_id": group_id})
    client.call("get_iam_policy", {"policy_id": policy_id}, error=True)
    client.call("get_iam_group", {"group_id": group_id}, error=True)
    tenant_client = MCP(app, member_key)
    tenant_client.call("list_members", error=True)
    tenant_client.call("list_api_keys", error=True)
    tenant_client.call("list_invites", error=True)
    tenant_client.call("tenant_retirement_inventory", error=True)
    invite_request = {"email": "iam-invite@example.invalid", "role": "member",
                      "expires_at": "2090-01-01T00:00:00Z"}
    tenant_client.call("create_invite", invite_request, error=True)
    for endpoint in ("/api/members", "/api/api-keys"):
        app.rest("GET", endpoint, status=403, key=member_key)
    admin_document = {"statements": [{"effect": "allow",
        "actions": ["tenant_get", "tenant_user_list", "tenant_user_create", "tenant_user_update", "tenant_user_delete", "credential_list",
                    "credential_create", "credential_rotate", "credential_revoke"],
        "resources": [{"type": "tenant", "tenant_id": app.tenant}]}]}
    delegated = client.call("create_iam_policy", {"name": "delegated-management", "document": admin_document})
    delegated_attachment = {"user_id": user_id, "policy_id": delegated["id"]}
    client.call("attach_user_policy", delegated_attachment)
    require(tenant_client.call("tenant_retirement_inventory") ==
            app.rest("GET", "/api/tenants/current/retirement", key=member_key),
            "delegated retirement inventory differs across transports")
    tenant_client.call("transfer_ownership", {"successor_user_id": user_id}, error=True)
    tenant_client.call("set_member_role", {"user_id": user_id, "role": "owner"}, error=True)
    tenant_client.call("remove_member", {"user_id": app.account["user_id"]}, error=True)
    admin_document["statements"].append({"effect": "allow", "actions": ["workspace_get", "workspace_update"],
        "resources": [{"type": "workspace", "tenant_id": app.tenant, "workspace_id": app.workspace}]})
    client.call("update_iam_policy", {"policy_id": delegated["id"], "name": "delegated-management", "document": admin_document})
    grant = {"workspace_id": app.workspace, "user_id": user_id}
    tenant_client.call("grant_workspace_member", {**grant, "role": "viewer"})
    require(tenant_client.call("list_workspace_members", {"workspace_id": app.workspace}) ==
            app.rest("GET", f"/api/workspaces/{app.workspace}/members", key=member_key),
            "delegated direct-grant listing differs across transports")
    invitation = tenant_client.call("create_invite", invite_request)
    app.secrets.append(invitation["token"])
    invitation_id = invitation["id"]
    require(tenant_client.call("list_invites") == app.rest("GET", "/api/members/invites", key=member_key),
            "delegated invite listing differs across transports")
    renewed = tenant_client.call("reissue_invite", {"invite_id": invitation_id,
                                                     "expires_at": invite_request["expires_at"]})
    app.secrets.append(renewed["token"])
    require(renewed["token"] != invitation["token"], "invite reissue retained the old token")
    tenant_client.call("create_invite", {**invite_request, "role": "owner"}, error=True)
    require(tenant_client.call("list_members") == app.rest("GET", "/api/members", key=member_key),
            "delegated member listing differs across transports")
    mcp_keys = tenant_client.call("list_api_keys")["keys"]
    rest_keys = app.rest("GET", "/api/api-keys", key=member_key)["keys"]
    require([{k: v for k, v in row.items() if k != "last_used_at"} for row in mcp_keys] ==
            [{k: v for k, v in row.items() if k != "last_used_at"} for row in rest_keys],
            "delegated credential listing differs across transports")
    delegated_id, delegated_key = app.issue(app.workspace, "read", issuer=member_key)
    replacement = tenant_client.call("rotate_api_key", {"key_id": delegated_id})
    app.secrets.append(replacement["full_key"])
    app.rest("GET", "/api/whoami", status=401, key=delegated_key)
    # The workspace-bound key must not administer tenant resources, even with an attached allow.
    MCP(app, replacement["full_key"]).call("list_api_keys", error=True)
    app.rest("GET", "/api/api-keys", status=403, key=replacement["full_key"])
    denied_admin = {"statements": [{**statement, "effect": "deny"} for statement in admin_document["statements"]]}
    client.call("update_iam_policy", {"policy_id": delegated["id"], "name": "delegated-management", "document": denied_admin})
    for tool, args in (("list_members", {}), ("list_api_keys", {}),
                       ("tenant_retirement_inventory", {}),
                       ("create_invite", invite_request), ("list_invites", {}),
                       ("reissue_invite", {"invite_id": invitation_id, "expires_at": invite_request["expires_at"]}),
                       ("revoke_invite", {"invite_id": invitation_id}),
                       ("revoke_workspace_member", grant),
                       ("preview_member_removal", {"user_id": user_id}),
                       ("revoke_api_key", {"key_id": replacement["id"]}),
                       ("set_member_role", {"user_id": user_id, "role": "member"})):
        tenant_client.call(tool, args, error=True)
    app.rest("DELETE", f"/api/api-keys/{replacement['id']}", status=403, key=member_key)
    app.rest("DELETE", f"/api/workspaces/{app.workspace}/members/{user_id}", status=403, key=member_key)
    app.rest("GET", "/api/whoami", key=replacement["full_key"])
    require(any(row["id"] == invitation_id and row["expires_at"] == renewed["expires_at"]
                for row in client.call("list_invites")), "denied invite mutation changed expiry")
    client.call("update_iam_policy", {"policy_id": delegated["id"], "name": "delegated-management", "document": admin_document})
    tenant_client.call("revoke_api_key", {"key_id": replacement["id"]})
    tenant_client.call("revoke_workspace_member", grant)
    app.rest("DELETE", f"/api/members/invites/{invitation_id}", status=204, key=member_key)
    app.rest("GET", "/api/whoami", status=401, key=replacement["full_key"])
    client.call("detach_user_policy", delegated_attachment)
    client.call("delete_iam_policy", {"policy_id": delegated["id"]})
    events = client.call("list_iam_audit", {"limit": 100})
    require(events and events == app.rest("GET", "/api/iam/audit?limit=100", key=app.tenant_key),
            "IAM audit REST/MCP mismatch")
    client.call("remove_member", {"user_id": user_id})


def human_key_management(app):
    human_owner(app)
    account, owner = app.account, app.tenant_key
    client = MCP(app, owner)

    expiry = "2090-01-01T00:00:00Z"
    original = app.rest("POST", "/api/api-keys", {
        "name": "bounded-read", "scope": "read", "expires_at": expiry, "tenant_scoped": True,
    }, status=201, key=owner)
    app.secrets.append(original["full_key"])
    reader = MCP(app, original["full_key"])
    require(reader.call("whoami")["user_id"] == account["user_id"], "delegated key lost attribution")
    for body in ({"name": "scope-escalation", "scope": "write", "expires_at": expiry},
                 {"name": "audit-escalation", "audit": True, "expires_at": expiry},
                 {"name": "unlimited"},
                 {"name": "longer", "expires_at": "2090-01-02T00:00:00Z"}):
        app.rest("POST", "/api/api-keys", {**body, "tenant_scoped": True}, status=403, key=original["full_key"])
        reader.call("create_api_key", {**body, "tenant_scoped": True}, error=True)
    successor = client.call("rotate_api_key", {"key_id": original["id"]})
    app.secrets.append(successor["full_key"])
    require(successor["id"] != original["id"] and successor["full_key"] != original["full_key"],
            "rotation did not replace the credential")
    for field in ("scope", "audit", "workspace_id", "tenant_id", "user_id", "expires_at"):
        require(original[field] == successor[field], f"rotation changed {field}")
    app.rest("GET", "/api/whoami", key=original["full_key"], status=401)
    reader.call("whoami", error=True)
    successor_client = MCP(app, successor["full_key"])
    require(successor_client.call("whoami")["scope"] == "read", "successor is unusable")
    app.rest("POST", f"/api/api-keys/{original['id']}/rotate", status=409, key=owner)
    app.rest("DELETE", f"/api/api-keys/{successor['id']}", status=204, key=owner)
    app.rest("GET", "/api/whoami", key=successor["full_key"], status=401)
    successor_client.call("whoami", error=True)
    for history in (app.rest("GET", "/api/api-keys", key=owner)["keys"], client.call("list_api_keys")["keys"]):
        records = {record["id"]: record for record in history}
        require(records[original["id"]]["replaced_by_id"] == successor["id"], "rotation history missing")
        for key in (original, successor):
            record = records[key["id"]]
            require(record["state"] == "revoked" and record["effective_scope"] is None,
                    "revoked key history is not terminal")
        serialized = json.dumps(history)
        require("full_key" not in serialized and "key_hash" not in serialized
                and all(key["full_key"] not in serialized for key in (original, successor)),
                "key history exposes secrets")


def template_library_lifecycle(app):
    owner = app.tenant_key
    client = MCP(app, owner)
    definition = {**SCHEMA, "name": "library-contract"}
    created = client.call("create_template_library_item", {
        "name": "library-contract", "definition": definition, "tags": ["blackbox"],
    })
    template_id = created["id"]
    path = f"/api/template-library/{template_id}"
    require(app.rest("GET", path, key=owner)["name"] == "library-contract",
            "MCP template creation not visible through REST")
    app.rest("PUT", path, {"name": "renamed-library", "tags": ["updated"]}, key=owner)
    require(client.call("get_template_library_item", {"id": template_id})["tags"] == ["updated"],
            "REST template update not visible through MCP")
    require(any(item["id"] == template_id for item in client.call("list_template_library")),
            "template not listed")
    schema = MCP(app, app.management_key).call("create_schema", {"template_id": template_id})["schema"]
    fork = client.call("fork_template_library_item", {"id": template_id, "name": "library-copy"})
    require(fork["id"] != template_id, "template fork reused source identity")
    client.call("update_template_library_item", {"id": template_id, "description": "changed after adoption"})
    retained = app.rest("GET", f"/api/schemas/{schema['id']}", key=app.management_key)
    require(retained["definition"] == schema["definition"], "library edit mutated adopted schema")
    client.call("delete_template_library_item", {"id": template_id})
    app.rest("GET", path, status=404, key=owner)
    client.call("get_template_library_item", {"id": template_id}, error=True)
    require(app.rest("GET", f"/api/schemas/{schema['id']}", key=app.management_key)["definition"] == retained["definition"],
            "template deletion removed adopted schema")
    app.rest("DELETE", f"/api/template-library/{fork['id']}", status=204, key=owner)
    require(not any(item["id"] in (template_id, fork["id"])
                    for item in client.call("list_template_library")), "deleted templates remain listed")


def user_profile_lifecycle(app):
    owner = app.management_key
    client = MCP(app, owner)
    path = "/api/users/me"
    original = client.call("get_current_user")
    require(app.rest("GET", path, key=owner) == original, "REST/MCP profiles differ")
    require(set(original) == {"id", "email", "tenant_id", "user_name", "display_name", "created_at"},
            "profile exposed private identity fields")
    updated = client.call("update_current_user", {"display_name": "  Contract Owner  "})
    require(updated["display_name"] == "Contract Owner" and updated["id"] == original["id"]
            and updated["email"] == original["email"], "profile update changed identity or ignored trimming")
    require(app.rest("GET", path, key=owner) == updated, "MCP profile update not visible to REST")
    for value in ("   ", "x" * 101):
        app.rest("PUT", path, {"display_name": value}, status=409, key=owner)
        client.call("update_current_user", {"display_name": value}, error=True)
    status, _, _ = app.http("PUT", path, {"display_name": "forbidden", "email": "other@example.invalid"},
                            key=owner)
    require(status == 422, "profile accepted an immutable identity field")
    client.call("update_current_user", {"display_name": "forbidden", "email": "other@example.invalid"}, error=True)
    require(client.call("get_current_user") == updated, "rejected update changed the profile")
    cleared = app.rest("PUT", path, {"display_name": None}, key=owner)
    require(cleared["display_name"] is None and client.call("get_current_user") == cleared,
            "REST profile clear not visible to MCP")


def invite_lifecycle(app):
    owner = app.tenant_key
    client = MCP(app, owner)
    path = "/api/members/invites"
    created = client.call("create_invite", {
        "email": "invited@example.invalid", "role": "viewer", "expires_at": "2090-01-01T00:00:00Z",
    })
    app.secrets.append(created["token"])
    invite_id = created["id"]
    replaced = app.rest("POST", f"{path}/{invite_id}/reissue", {
        "expires_at": "2090-01-02T00:00:00Z",
    }, key=owner)
    app.secrets.append(replaced["token"])
    require(replaced["id"] == invite_id and replaced["token"] != created["token"],
            "invite reissue did not replace the token on the same record")
    password = "blackbox-invited-password"
    app.secrets.append(password)
    app.rest("POST", "/auth/signup", {"invite_token": created["token"], "password": password}, status=422)
    records = client.call("list_invites")
    require(records == app.rest("GET", path, key=owner), "REST/MCP invite history differs")
    require(all("token" not in row and "token_hash" not in row for row in records)
            and all(secret not in json.dumps(records) for secret in (created["token"], replaced["token"])),
            "invite listing exposed a credential")
    accepted = app.rest("POST", "/auth/signup", {
        "invite_token": replaced["token"], "password": password,
    }, status=201)
    require(accepted["tenant_id"] == app.tenant and accepted["role"] == "viewer", "invite redemption lost grants")
    app.rest("POST", "/auth/signup", {"invite_token": replaced["token"], "password": password}, status=422)
    app.rest("DELETE", f"{path}/{invite_id}", status=409, key=owner)
    client.call("reissue_invite", {"invite_id": invite_id, "expires_at": "2090-01-03T00:00:00Z"}, error=True)
    used = next(row for row in client.call("list_invites") if row["id"] == invite_id)
    require(used["used_at"] is not None, "consumed invitation history lost")
    cancelled = app.rest("POST", path, {
        "email": "cancelled@example.invalid", "role": "viewer", "expires_at": "2090-01-01T00:00:00Z",
    }, status=201, key=owner)
    app.secrets.append(cancelled["token"])
    client.call("revoke_invite", {"invite_id": cancelled["id"]})
    app.rest("DELETE", f"{path}/{cancelled['id']}", status=204, key=owner)
    app.rest("POST", "/auth/signup", {"invite_token": cancelled["token"], "password": password}, status=422)
    require(any(row["id"] == cancelled["id"] and row["used_at"] is None for row in client.call("list_invites")),
            "cancelled invitation history was deleted")
    client.call("create_invite", {
        "email": "owner@example.invalid", "role": "owner", "expires_at": "2090-01-01T00:00:00Z",
    }, error=True)
    existing = client.call("create_invite", {
        "email": "owner@example.invalid", "role": "member", "expires_at": "2090-01-01T00:00:00Z",
    })
    app.secrets.append(existing["token"])
    refreshed = client.call("reissue_invite", {
        "invite_id": existing["id"], "expires_at": "2090-01-02T00:00:00Z",
    })
    app.secrets.append(refreshed["token"])
    require(refreshed["id"] == existing["id"] and refreshed["token"] != existing["token"],
            "MCP invite reissue failed to replace the token")
    identity = client.call("whoami")
    client.call("accept_invite", {"invite_token": existing["token"]}, error=True)
    client.call("accept_invite", {"invite_token": refreshed["token"]}, error=True)
    require(next(row for row in client.call("list_invites") if row["id"] == existing["id"])["used_at"] is None,
            "rejected existing membership consumed the invitation")
    require(client.call("whoami") == identity, "invite acceptance changed selected workspace or credentials")


def audit_history_lifecycle(app):
    owner = app.management_key
    client = MCP(app, owner)
    client.call("list_audit_log", error=True)
    app.rest("GET", "/api/audit-log", key=owner, status=403)
    key_id, key = app.issue(app.workspace, "read", audit=True)
    auditor = MCP(app, key)
    capabilities = auditor.call("capabilities")
    require(capabilities["audit"] and "schema" not in capabilities["scopes"],
            "audit grant incorrectly elevated ordinary scope")
    auditor.call("create_schema", {"definition": {**SCHEMA, "name": "audit-denied"}}, error=True)
    _, migration_key = app.issue(app.workspace, "migration")
    migration = MCP(app, migration_key)
    migration.call("reindex_workspace")
    records = auditor.call("list_audit_log", {"page": 1, "page_size": 50})
    require(records and records == app.rest("GET", "/api/audit-log?page=1&page_size=50", key=key),
            "audit history empty or differs across transports")
    require(any(row["action"] == "reindex_embeddings" for row in records),
            "queued reindex did not leave its audit record")
    require(all(row["workspace_id"] == app.workspace and row["tenant_id"] == app.tenant for row in records),
            "audit history crossed workspace or tenant boundary")
    serialized = json.dumps(records)
    require(all(secret not in serialized for secret in app.secrets), "audit history exposed a credential")
    first = auditor.call("list_audit_log", {"page": 1, "page_size": 1})
    require(first == records[:1], "audit pagination changed first record")
    require(auditor.call("list_audit_log", {"page": 0, "page_size": 1}) == first,
            "zero audit page did not normalize to the first page")
    auditor.call("list_audit_log", {"page": 18446744073709551615, "page_size": 50}, error=True)
    app.rest("DELETE", f"/api/api-keys/{key_id}", status=204, key=app.tenant_key)
    auditor.call("list_audit_log", error=True)
    app.rest("GET", "/api/audit-log", key=key, status=401)


def workspace_key_lifecycle(app):
    owner = app.management_key
    client = MCP(app, owner)
    identity = client.call("whoami")
    login = app.rest("POST", "/auth/root/login", {
        "email": "owner@example.invalid", "password": "blackbox-owned-password",
    })
    tenant_key = login["api_key"]
    app.secrets.append(tenant_key)
    tenant_client = MCP(app, tenant_key)
    capabilities = client.call("capabilities")
    require(capabilities["tenant_root"] and not capabilities["constraints"]["discovery_authorizes_requests"],
            "capabilities confused discovery with authorization")
    client.call("create_api_key", {"name": "bound-denied", "scope": "read"}, error=True)
    app.rest("POST", "/api/api-keys", {"name": "bound-denied", "scope": "read"}, status=403, key=owner)
    from datetime import UTC, datetime, timedelta
    expiry = (datetime.now(UTC) + timedelta(hours=1)).isoformat()
    created = tenant_client.call("create_api_key", {"name": "mcp-issued", "scope": "read", "tenant_scoped": True,
                                                    "expires_at": expiry})
    app.secrets.append(created["full_key"])
    delegated = MCP(app, created["full_key"])
    require(delegated.call("whoami")["user_id"] == identity["user_id"], "MCP key lost attribution")
    client.call("revoke_api_key", {"key_id": created["id"]}, error=True)
    tenant_client.call("revoke_api_key", {"key_id": created["id"]})
    delegated.call("whoami", error=True)
    app.rest("GET", "/api/whoami", key=created["full_key"], status=401)
    workspace = tenant_client.call("create_workspace", {"name": "mcp-managed"})
    workspace_id = workspace["id"]
    require(any(row["id"] == workspace_id for row in tenant_client.call("list_workspaces")),
            "MCP-created workspace not listed for tenant credential")
    client.call("list_workspaces", error=True)
    app.rest("GET", "/api/workspaces", status=403, key=owner)
    require(any(row["workspace_id"] == workspace_id and row["tenant_id"] == app.tenant
                for row in tenant_client.call("discover_workspaces")),
            "human discovery omitted tenant workspace")
    _, selected_key = app.issue(workspace_id, "schema", issuer=tenant_key, expires_at=expiry)
    selected = MCP(app, selected_key)
    selected.call("update_workspace", {"workspace_id": workspace_id, "replace_max_entities": True,
                                       "max_entities": 5}, error=True)
    tenant_client.call("update_workspace", {"workspace_id": workspace_id, "name": "mcp-renamed",
                                       "replace_max_entities": True, "max_entities": 5})
    detail = selected.call("get_workspace", {"workspace_id": workspace_id})
    require(detail["name"] == "mcp-renamed" and detail["max_entities"] == 5,
            "MCP workspace update not persisted")
    require(app.rest("GET", f"/api/workspaces/{workspace_id}", key=selected_key) == detail,
            "workspace detail differs across transports")
    args = {"workspace_id": workspace_id, "user_id": identity["user_id"]}
    tenant_client.call("grant_workspace_member", {**args, "role": "viewer"})
    require(any(row["user_id"] == identity["user_id"] for row in tenant_client.call("list_workspace_members", {
        "workspace_id": workspace_id,
    })), "MCP workspace grant missing")
    tenant_client.call("revoke_workspace_member", args)
    require(not any(row["user_id"] == identity["user_id"] for row in tenant_client.call("list_workspace_members", {
        "workspace_id": workspace_id,
    })), "MCP workspace grant survived revocation")
    tenant_client.call("delete_workspace", {"workspace_id": workspace_id})
    app.rest("GET", f"/api/workspaces/{workspace_id}", key=owner, status=404)
    selected.call("whoami", error=True)
    require(client.call("whoami")["user_id"] == identity["user_id"], "workspace deletion invalidated unrelated key")


def membership_retirement_lifecycle(app):
    owner = app.tenant_key
    client = MCP(app, owner)
    owner_id = client.call("whoami")["user_id"]
    for name, args in (("preview_member_removal", {"user_id": owner_id}),
                       ("preview_member_role_change", {"user_id": owner_id, "role": "admin"})):
        require(client.call(name, args)["blocked_by_last_owner"], "preview missed last-owner protection")
    client.call("remove_member", {"user_id": owner_id}, error=True)
    client.call("set_member_role", {"user_id": owner_id, "role": "admin"}, error=True)
    require(next(row for row in client.call("list_members") if row["user_id"] == owner_id)["role"] == "owner",
            "rejected last-owner change altered membership")
    password = "blackbox-successor-password"
    app.secrets.append(password)
    account = app.rest("POST", "/api/members/users", {
        "user_name": "successor", "password": password, "role": "admin",
    }, status=201, key=owner)
    user_id = account["user_id"]
    client.call("grant_workspace_member", {
        "workspace_id": app.workspace, "user_id": user_id, "role": "viewer",
    })
    member_name = next(row["user_name"] for row in client.call("list_members") if row["user_id"] == user_id)
    login = app.rest("POST", "/auth/tenant/login", {"user_name": member_name, "password": password},
                     key="", headers={"X-Tenant-Id": app.tenant})
    member_login = login["api_key"]
    app.secrets.append(member_login)
    key = member_login
    member = MCP(app, key)
    before = client.call("list_members")
    require(before == app.rest("GET", "/api/members", key=owner), "membership lists differ")
    preview = client.call("preview_member_role_change", {"user_id": user_id, "role": "viewer"})
    require(preview["current_role"] == "admin" and preview["requested_role"] == "viewer"
            and not preview["blocked_by_last_owner"], "wrong member role preview")
    stored_scope = member.call("whoami")["stored_scope"]
    require(client.call("list_members") == before,
            "role preview mutated membership or grants")
    client.call("set_member_role", {"user_id": user_id, "role": "viewer"})
    require(member.call("whoami")["stored_scope"] == stored_scope,
            "legacy role change rewrote the credential ceiling")
    member.call("update_tenant", {"name": "viewer-denied"}, error=True)
    removal = client.call("preview_member_removal", {"user_id": user_id})
    require(removal == app.rest("GET", f"/api/members/{user_id}/removal-preview", key=owner)
            and not removal["blocked_by_last_owner"] and removal["revoked_api_key_ids"],
            "removal preview lost credential impact")
    require(member.call("whoami")["user_id"] == user_id, "removal preview revoked a credential")
    client.call("remove_member", {"user_id": user_id})
    app.rest("DELETE", f"/api/members/{user_id}", status=204, key=owner)
    app.rest("GET", "/api/whoami", status=401, key=key)
    member.call("whoami", error=True)
    require(not any(row["user_id"] == user_id for row in client.call("list_members")),
            "removed membership remains visible")
    app.rest("GET", "/api/whoami", status=401, key=key)
    replacement = app.rest("POST", "/api/members/users", {
        "user_name": "replacement-successor", "password": password, "role": "admin",
    }, status=201, key=owner)
    user_id = replacement["user_id"]
    member_name = replacement["user_name"]
    successor_key = app.rest("POST", "/auth/tenant/login", {"user_name": member_name, "password": password},
                             key="", headers={"X-Tenant-Id": app.tenant})["api_key"]
    app.secrets.append(successor_key)
    successor = MCP(app, successor_key)
    client.call("transfer_ownership", {"successor_user_id": user_id}, error=True)
    roles = {row["user_id"]: row["role"] for row in client.call("list_members")}
    require(roles[user_id] == "admin" and roles[owner_id] == "owner", "root transfer changed membership")
    successor.call("tenant_retirement_inventory", error=True)
    root_recovery(app)
    new_key = app.login("owner@example.invalid", "blackbox-recovered-password")
    client = MCP(app, new_key)
    inventory = client.call("tenant_retirement_inventory")
    require(inventory == app.rest("GET", "/api/tenants/current/retirement", key=new_key)
            and inventory["tenant_id"] == app.tenant and inventory["disabled_at"] is None
            and app.workspace in inventory["workspace_ids"], "wrong retirement inventory")
    client.call("disable_tenant", {"confirmed_tenant_id": str(uuid.uuid4())}, error=True)
    require(client.call("tenant_retirement_inventory") == inventory, "bad confirmation changed tenant")
    client.call("disable_tenant", {"confirmed_tenant_id": app.tenant})
    for credential, session in ((new_key, client), (successor_key, successor)):
        app.rest("GET", "/api/whoami", status=401, key=credential)
        session.call("whoami", error=True)
    out, _ = app.cli("task", "delete_tenant", f"tenant_id:{app.tenant}",
                     f"confirmed_tenant_id:{app.tenant}")
    require('"status":"deleted"' in out.replace(" ", ""), "tenant deletion did not complete")


def tenant_settings_lifecycle(app):
    bound = app.management_key
    MCP(app, bound).call("update_tenant", {"name": "Bound key"}, error=True)
    app.rest("PUT", "/api/tenants/current", {"name": "Bound key"}, key=bound, status=403)
    owner = app.tenant_key
    client = MCP(app, owner)
    path = "/api/tenants/current"
    original = client.call("get_tenant")
    require(original["id"] == app.tenant and app.rest("GET", path, key=owner) == original,
            "tenant metadata differs across transports or tenant boundary")
    updated = client.call("update_tenant", {
        "name": "  Contract Tenant  ", "replace_max_workspaces": True, "max_workspaces": 100,
    })
    require(updated["name"] == "Contract Tenant" and updated["max_workspaces"] == 100
            and updated["id"] == original["id"] and updated["created_at"] == original["created_at"],
            "tenant update changed identity or ignored metadata")
    require(app.rest("GET", path, key=owner) == updated, "MCP tenant update not visible to REST")
    renamed = app.rest("PUT", path, {"name": "Renamed Tenant"}, key=owner)
    require(renamed["max_workspaces"] == 100 and client.call("get_tenant") == renamed,
            "name-only update cleared cap or was not visible to MCP")
    for body in ({"name": "   "}, {"name": "x" * 101}, {"max_workspaces": 3},
                 {"replace_max_workspaces": True, "max_workspaces": -1}):
        app.rest("PUT", path, body, status=409, key=owner)
        client.call("update_tenant", body, error=True)
    require(client.call("get_tenant") == renamed, "invalid tenant update changed stored metadata")
    cleared = app.rest("PUT", path, {"replace_max_workspaces": True, "max_workspaces": None}, key=owner)
    require(cleared["max_workspaces"] is None and client.call("get_tenant") == cleared,
            "explicit tenant cap clear failed")


def migration_lifecycle(app):
    _, key = app.issue(app.workspace, "migration")
    client = MCP(app, key)
    name = "migration-contract"
    definition = {**SCHEMA, "name": name}
    first = client.call("create_schema", {"definition": definition})["schema"]
    entity = client.call("create_entity", {
        "schema_name": name, "entity_type": "note", "data": {"title": "before"},
    })
    definition = {**definition, "entity_types": {"note": {"fields": {
        **SCHEMA["entity_types"]["note"]["fields"],
        "priority": {"type": "integer", "required": True, "default": 7},
    }}}}
    active = client.call("create_schema", {"definition": definition})["schema"]
    require(active["version"] > first["version"], "migration schema did not evolve")
    drift = client.call("get_entity_drift", {"id": entity["id"]})
    require(any(field["name"] == "priority" and field["required"] for field in drift["missing_fields"]),
            "drift omitted required default field")
    report = client.call("migration_dry_run", {"name": name})
    require(report["total_entities"] == 1 and report["needs_values"] == 1,
            "dry-run did not count the old entity")
    require(client.call("get_entity", {"id": entity["id"]})["data"] == entity["data"],
            "dry-run changed entity data")
    reader = MCP(app, app.key)
    reader.call("fill_defaults", {"schema_name": name}, error=True)
    job_id = client.call("fill_defaults", {"schema_name": name})["job_id"]
    require(client.call("migration_job_status", {"job_id": job_id})["status"] == "queued",
            "migration was not queued before worker startup")
    require(client.call("get_entity", {"id": entity["id"]})["data"] == entity["data"],
            "enqueue changed entity data before worker startup")

    def completed(job_id, worker):
        while True:
            app.remaining()
            require(worker.poll() is None and app.server.poll() is None, "migration process exited")
            job = client.call("migration_job_status", {"job_id": job_id})
            require(job["status"] in ("queued", "running", "completed"), f"migration failed: {job}")
            if job["status"] == "completed":
                require(app.rest("GET", f"/api/migration-jobs/{job_id}", key=key) == job,
                        "REST/MCP migration status differs")
                return
            time.sleep(min(0.1, app.remaining()))

    with app.job_worker() as worker:
        completed(job_id, worker)
        filled = client.call("get_entity", {"id": entity["id"]})
        require(filled["data"] == {"title": "before", "priority": 7}
                and filled["schema_version"] == entity["schema_version"],
                "fill-defaults changed version or failed to fill data")
        reader.call("undo_migration_job", {"job_id": job_id}, error=True)
        undo = app.rest("POST", f"/api/migration-jobs/{job_id}/undo", status=202, key=key)
        require(undo["snapshot_job_id"] == job_id and undo["job_id"] != job_id, "undo identity lost")
        completed(undo["job_id"], worker)
        second_job = client.call("fill_defaults", {"schema_name": name})["job_id"]
        completed(second_job, worker)
        require(client.call("get_entity", {"id": entity["id"]})["data"] == filled["data"],
                "second migration did not refill restored data")
        second_undo = client.call("undo_migration_job", {"job_id": second_job})
        require(second_undo["snapshot_job_id"] == second_job and second_undo["job_id"] != second_job,
                "MCP undo lost snapshot identity")
        completed(second_undo["job_id"], worker)
    restored = client.call("get_entity", {"id": entity["id"]})
    require(restored["data"] == entity["data"] and restored["schema_version"] == entity["schema_version"],
            "undo did not restore pre-migration snapshot")
    reader.call("reindex_workspace", error=True)
    reindex = client.call("reindex_workspace")
    queued = client.call("migration_job_status", {"job_id": reindex["job_id"]})
    require(queued["job_name"] == "reindex" and queued["status"] == "queued"
            and app.rest("GET", f"/api/migration-jobs/{reindex['job_id']}", key=key) == queued,
            "reindex enqueue lost job identity or cross-transport status")


def enterprise_settings(app):
    client = MCP(app, app.key)
    _, read_key = app.issue(app.workspace, "read")
    reader = MCP(app, read_key)
    path = "/api/workspace/entity-columns"
    saved = client.call("set_entity_columns", {"entity_type": "note", "columns": ["title", "body"]})
    require(saved == {"entity_type": "note", "columns": ["title", "body"]}, "wrong stored columns")
    listed = reader.call("list_entity_columns")
    require(listed == app.rest("GET", path), "column preferences differ across transports")
    client.call("set_entity_columns", {"entity_type": "note", "columns": ["title", "title"]}, error=True)
    reader.call("set_entity_columns", {"entity_type": "note", "columns": []}, error=True)
    reader.call("reset_entity_columns", {"entity_type": "note"}, error=True)
    require(client.call("list_entity_columns") == listed, "rejected column writes mutated preferences")
    require(client.call("reset_entity_columns", {"entity_type": "note"})["reset"], "column reset failed")
    require(app.rest("GET", path) == [], "reset left column preferences")
    app.rest("PUT", path + "/note", {"columns": []})
    require(client.call("list_entity_columns") == [{"entity_type": "note", "columns": []}],
            "empty preferences were confused with reset")
    app.rest("DELETE", path + "/note", status=204)
    require(reader.call("list_entity_columns") == [], "REST reset did not reach MCP")

    path = "/api/workspace/worker-class"
    client.call("get_worker_class", error=True)
    require(client.call("set_worker_class", {"worker_class": "shared"})["saved"], "worker assignment failed")
    stored = reader.call("get_worker_class")
    require(stored["worker_class"] == "shared" and stored == app.rest("GET", path),
            "worker assignment differs across transports")
    reader.call("set_worker_class", {"worker_class": "shared"}, error=True)
    reader.call("reset_worker_class", error=True)
    require(client.call("get_worker_class") == stored, "denied reset changed worker routing")
    require(client.call("reset_worker_class")["reset"], "worker reset failed")
    app.rest("GET", path, status=404)
    client.call("get_worker_class", error=True)
    app.rest("PUT", path, {"worker_class": "shared"}, status=204)
    require(reader.call("get_worker_class")["worker_class"] == "shared", "REST assignment did not reach MCP")
    app.rest("DELETE", path, status=204)
    client.call("get_worker_class", error=True)


def enterprise_providers(app):
    client = MCP(app, app.key)
    _, read_key = app.issue(app.workspace, "read")
    reader = MCP(app, read_key)
    for kind in ("llm", "embedding"):
        path = f"/api/workspace/{kind}-key"
        secret = f"blackbox-{kind}-provider-secret"
        app.secrets.append(secret)
        settings = {"base_url": f"https://{kind}.example.invalid/v1",
                    "model": f"test-{kind}", "api_key": secret}
        if kind == "embedding":
            settings["dimensions"] = 32
        get, put, reset = (f"{operation}_{kind}_provider" for operation in ("get", "set", "reset"))
        client.call(get, error=True)
        app.rest("GET", path, status=404)
        saved = client.call(put, settings)
        require(saved == {"saved": True}, "provider configuration failed")
        metadata = reader.call(get)
        require(metadata == app.rest("GET", path) and metadata["model"] == settings["model"],
                "provider metadata differs across transports")
        require("api_key" not in metadata and secret not in json.dumps([saved, metadata]),
                "provider metadata disclosed its secret")
        reader.call(put, settings, error=True)
        reader.call(reset, error=True)
        invalid = dict(settings)
        invalid["api_key" if kind == "llm" else "dimensions"] = "" if kind == "llm" else 0
        client.call(put, invalid, error=True)
        require(reader.call(get) == metadata, "rejected provider writes mutated configuration")
        require(client.call(reset) == {"reset": True}, "provider reset failed")
        client.call(get, error=True)
        app.rest("GET", path, status=404)
        app.rest("PUT", path, settings, status=204)
        require(reader.call(get) == metadata, "REST provider configuration did not reach MCP")
        app.rest("DELETE", path, status=204)
        client.call(get, error=True)
        app.rest("GET", path, status=404)

    root = MCP(app, app.tenant_key)
    password = "blackbox-provider-admin-password"
    app.secrets.append(password)
    member = app.rest("POST", "/api/members/users", {
        "user_name": "provider-admin", "password": password, "role": "admin",
    }, status=201, key=app.tenant_key)
    members = app.rest("GET", "/api/members", key=app.tenant_key)
    name = next(row["user_name"] for row in members if row["user_id"] == member["user_id"])
    login = app.rest("POST", "/auth/tenant/login", {"user_name": name, "password": password},
                     key="", headers={"X-Tenant-Id": app.tenant})
    app.secrets.append(login["api_key"])
    issuance = root.call("create_iam_policy", {"name": "provider-key-issuance", "document": {"statements": [
        {"effect": "allow", "actions": ["credential_create"],
         "resources": [{"type": "tenant", "tenant_id": app.tenant}]}]}})
    root.call("attach_user_policy", {"user_id": member["user_id"], "policy_id": issuance["id"]})
    _, key = app.issue(app.workspace, "schema", issuer=login["api_key"])
    root.call("delete_iam_policy", {"policy_id": issuance["id"]})
    ordinary = MCP(app, key)
    path = "/api/workspace/llm-key"
    settings = {"base_url": "https://llm.example.invalid/v1", "model": "iam-model", "api_key": "iam-secret"}
    app.secrets.append(settings["api_key"])
    setting_cases = [
        ("/api/workspace/embedding-key", "get_embedding_provider", "set_embedding_provider",
         "reset_embedding_provider", {**settings, "dimensions": 8}),
        ("/api/workspace/worker-class", "get_worker_class", "set_worker_class",
         "reset_worker_class", {"worker_class": "shared"}),
        ("/api/workspace/entity-columns", "list_entity_columns", "set_entity_columns",
         "reset_entity_columns", {"entity_type": "note", "columns": ["title"]}),
    ]
    for endpoint, get, put, reset, body in setting_cases:
        ordinary.call(get, error=True)
        ordinary.call(put, body, error=True)
        ordinary.call(reset, {"entity_type": "note"} if get == "list_entity_columns" else {}, error=True)
        app.rest("GET", endpoint, status=403, key=key)
    for method, body in (("GET", None), ("PUT", settings), ("DELETE", None)):
        app.rest(method, path, body, status=403, key=key)
    ordinary.call("set_llm_provider", settings, error=True)
    document = {"statements": [{"effect": "allow",
        "actions": ["llm_provider_get", "llm_provider_update", "llm_provider_delete"],
        "resources": [{"type": "workspace", "tenant_id": app.tenant, "workspace_id": app.workspace}]}]}
    policy = root.call("create_iam_policy", {"name": "provider-admin", "document": document})
    attachment = {"user_id": member["user_id"], "policy_id": policy["id"]}
    root.call("attach_user_policy", attachment)
    ordinary.call("list_schema_forks", error=True)
    ordinary.call("list_upstream_changes", error=True)
    missing_schema = str(uuid.uuid4())
    ordinary.call("merge_preview", {"schema_id": missing_schema}, error=True)
    ordinary.call("merge_apply", {"schema_id": missing_schema}, error=True)
    app.rest("GET", "/api/schemas/upstream-changes", status=403, key=key)
    app.rest("GET", f"/api/schemas/{missing_schema}/merge-preview", status=403, key=key)
    app.rest("POST", f"/api/schemas/{missing_schema}/merge", status=403, key=key)
    app.rest("GET", "/api/schema-forks", status=403, key=key)
    document["statements"][0]["actions"].append("schema_fork_list")
    for prefix in ("embedding_provider", "worker_class", "entity_columns"):
        document["statements"][0]["actions"].extend(f"{prefix}_{action}" for action in ("get", "update", "delete"))
    document["statements"][0]["actions"].extend(["schema_upstream_list", "schema_merge_preview", "schema_merge_apply"])
    root.call("update_iam_policy", {"policy_id": policy["id"], "name": "provider-admin", "document": document})
    require(ordinary.call("list_schema_forks") == app.rest("GET", "/api/schema-forks", key=key),
            "delegated schema fork list differs across transports")
    require(ordinary.call("list_upstream_changes") == app.rest("GET", "/api/schemas/upstream-changes", key=key),
            "delegated upstream list differs across transports")
    app.rest("GET", f"/api/schemas/{missing_schema}/merge-preview", status=404, key=key)
    app.rest("POST", f"/api/schemas/{missing_schema}/merge", status=404, key=key)
    ordinary.call("set_llm_provider", settings)
    stored = ordinary.call("get_llm_provider")
    stored_settings = []
    for endpoint, get, put, _, body in setting_cases:
        ordinary.call(put, body)
        record = ordinary.call(get)
        require(record == app.rest("GET", endpoint, key=key), "delegated settings transport mismatch")
        stored_settings.append(record)
    require(stored == app.rest("GET", path, key=key), "delegated LLM metadata differs")
    deny = {"statements": [{**document["statements"][0], "effect": "deny"}]}
    root.call("update_iam_policy", {"policy_id": policy["id"], "name": "provider-admin", "document": deny})
    for (endpoint, get, put, reset, body), record in zip(setting_cases, stored_settings):
        args = {"entity_type": "note"} if get == "list_entity_columns" else {}
        ordinary.call(get, error=True)
        ordinary.call(put, body, error=True)
        ordinary.call(reset, args, error=True)
        mutation_path = endpoint + "/note" if get == "list_entity_columns" else endpoint
        app.rest("PUT", mutation_path, body, status=403, key=key)
        app.rest("DELETE", mutation_path, status=403, key=key)
        require(client.call(get) == record, "denied settings mutation changed persisted state")
    for tool, args in (("get_llm_provider", {}), ("set_llm_provider", settings), ("reset_llm_provider", {})):
        ordinary.call(tool, args, error=True)
    for method, body in (("GET", None), ("PUT", settings), ("DELETE", None)):
        app.rest(method, path, body, status=403, key=key)
    ordinary.call("list_schema_forks", error=True)
    app.rest("GET", "/api/schema-forks", status=403, key=key)
    ordinary.call("list_upstream_changes", error=True)
    app.rest("GET", "/api/schemas/upstream-changes", status=403, key=key)
    app.rest("GET", f"/api/schemas/{missing_schema}/merge-preview", status=403, key=key)
    app.rest("POST", f"/api/schemas/{missing_schema}/merge", status=403, key=key)
    require(client.call("get_llm_provider") == stored, "denied LLM operation mutated settings")
    root.call("update_iam_policy", {"policy_id": policy["id"], "name": "provider-admin", "document": document})
    ordinary.call("reset_llm_provider")
    for _, get, _, reset, _ in setting_cases:
        ordinary.call(reset, {"entity_type": "note"} if get == "list_entity_columns" else {})
    root.call("detach_user_policy", attachment)
    ordinary.call("set_llm_provider", settings, error=True)
    for endpoint, get, _, _, _ in setting_cases:
        ordinary.call(get, error=True)
        app.rest("GET", endpoint, status=403, key=key)
    tenant_member = MCP(app, login["api_key"])
    tenant_member.call("get_tenant_overview", error=True)
    tenant_document = {"statements": [{"effect": "allow", "actions": ["tenant_get"],
        "resources": [{"type": "tenant", "tenant_id": app.tenant}]}]}
    root.call("update_iam_policy", {"policy_id": policy["id"], "name": "provider-admin", "document": tenant_document})
    root.call("attach_user_policy", attachment)
    app.rest("GET", "/api/tenant/overview", status=403, key=login["api_key"])
    tenant_document["statements"][0]["actions"].append("tenant_user_list")
    root.call("update_iam_policy", {"policy_id": policy["id"], "name": "provider-admin", "document": tenant_document})
    require(tenant_member.call("get_tenant_overview") == app.rest("GET", "/api/tenant/overview", key=login["api_key"]),
            "delegated tenant overview differs across transports")
    tenant_document["statements"].append({"effect": "deny", "actions": ["tenant_user_list"],
        "resources": [{"type": "tenant", "tenant_id": app.tenant}]})
    root.call("update_iam_policy", {"policy_id": policy["id"], "name": "provider-admin", "document": tenant_document})
    tenant_member.call("get_tenant_overview", error=True)
    app.rest("GET", "/api/tenant/overview", status=403, key=login["api_key"])
    root.call("delete_iam_policy", {"policy_id": policy["id"]})
    root.call("remove_member", {"user_id": member["user_id"]})


def enterprise_schema_composition(app):
    _, owner_key = human_owner(app)
    client = MCP(app, owner_key)
    administrator = MCP(app, app.tenant_key)
    overview = administrator.call("get_tenant_overview")
    require(overview == app.rest("GET", "/api/tenant/overview", key=app.tenant_key),
            "tenant overview differs across transports")
    MCP(app, app.key).call("get_tenant_overview", error=True)
    app.rest("GET", "/api/tenant/overview", status=403, key=app.key)
    _, read_key = app.issue(app.workspace, "read")
    reader = MCP(app, read_key)
    definition = json.loads(json.dumps(SCHEMA))
    definition["name"] = "upstream-note"
    template = administrator.call("create_template_library_item", {"name": "upstream-note", "definition": definition})
    source = client.call("create_schema", {"template_id": template["id"]})["schema"]
    original = client.call("get_schema_by_id", {"schema_id": source["id"]})
    evolved = json.loads(json.dumps(definition))
    evolved["entity_types"]["note"]["fields"]["category"] = {"type": "string"}
    administrator.call("update_template_library_item", {"id": template["id"], "definition": evolved})
    pending = {**original, "origin_updated_at": None}
    require(client.call("get_schema_by_id", {"schema_id": source["id"]}) == pending,
            "upstream notification changed more than its pending flag")
    changes = reader.call("list_upstream_changes")
    require(len(changes) == 1 and changes == app.rest("GET", "/api/schemas/upstream-changes", key=owner_key),
            "upstream discovery differs across transports")
    preview = reader.call("merge_preview", {"schema_id": source["id"]})
    require(preview["summary"]["auto_add"] == 1
            and preview == app.rest("GET", f"/api/schemas/{source['id']}/merge-preview", key=owner_key),
            "merge preview differs across transports")
    reader.call("merge_apply", {"schema_id": source["id"]}, error=True)
    require(client.call("get_schema_by_id", {"schema_id": source["id"]}) == pending, "preview or denial mutated source")
    merged = client.call("merge_apply", {"schema_id": source["id"]})["schema"]
    require(merged["version"] == source["version"] + 1, "merge did not create a new schema version")
    require(reader.call("list_upstream_changes") == [], "merged origin still reports pending changes")
    archived = {**pending, "status": "archived"}
    require(client.call("get_schema_by_id", {"schema_id": source["id"]}) == archived, "merge changed immutable history")

    target = app.create_workspace("fork-target")
    _, key = app.issue(target, "schema")
    _, target_read = app.issue(target, "read")
    owner, viewer = MCP(app, key), MCP(app, target_read)
    input_ = {"source_workspace_id": app.workspace, "source_schema_id": merged["id"]}
    viewer.call("create_schema_fork", input_, error=True)
    fork = owner.call("create_schema_fork", input_)
    path = f"/api/schema-forks/{fork['id']}"
    require(fork == app.rest("GET", path, key=key), "fork differs across transports")
    require(viewer.call("list_schema_forks") == [fork], "fork missing from target list")
    client.call("get_schema_fork", {"fork_id": fork["id"]}, error=True)
    update = {"fork_id": fork["id"], "definition": fork["definition"],
              "expected_fork_schema_id": "00000000-0000-0000-0000-000000000001"}
    owner.call("update_schema_fork", update, error=True)
    require(owner.call("get_schema_fork", {"fork_id": fork["id"]}) == fork, "stale update mutated fork")
    update["expected_fork_schema_id"] = fork["fork_schema_id"]
    updated = owner.call("update_schema_fork", update)
    require(updated["id"] == fork["id"] and updated == app.rest("GET", path, key=key),
            "updated fork differs across transports")
    viewer.call("delete_schema_fork", {"fork_id": fork["id"]}, error=True)
    require(owner.call("delete_schema_fork", {"fork_id": fork["id"]})["deleted"], "fork deletion failed")
    app.rest("GET", path, status=404, key=key)
    require(viewer.call("list_schema_forks") == [], "deleted fork remains visible")
    require(client.call("get_schema_by_id", {"schema_id": source["id"]}) == archived, "fork lifecycle changed source history")


@contextlib.contextmanager
def embedding_endpoint():
    requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            document = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            require(self.path == "/embeddings" and document["model"] == "blackbox-owned",
                    "unexpected embedding request")
            require(self.headers.get("Authorization") == "Bearer blackbox-provider-secret",
                    "embedding credential missing")
            requests.extend(document["input"])
            data = [{"index": index, "embedding": [1.0] + [0.0] * 767}
                    for index, _ in enumerate(document["input"])]
            body = json.dumps({"data": data}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_args):
            pass

    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}", requests
        finally:
            server.shutdown()
            thread.join(timeout=5)
            require(not thread.is_alive(), "embedding endpoint did not stop")


def enterprise_search(app):
    workspace = app.create_workspace("search-target")
    _, key = app.issue(workspace, "migration")
    _, schema_key = app.issue(workspace, "schema")
    client, writer = MCP(app, key), MCP(app, schema_key)
    definition = json.loads(json.dumps(SCHEMA))
    definition["name"] = "search-notes"
    definition["entity_types"]["note"]["fields"]["title"]["x-embed"] = True
    writer.call("create_schema", {"definition": definition})
    entity = writer.call("create_entity", {"schema_name": definition["name"], "entity_type": "note",
                                           "data": {"title": "owned search document"}})
    secret = "blackbox-provider-secret"
    app.secrets.append(secret)
    with embedding_endpoint() as (url, requests):
        writer.call("set_embedding_provider", {"base_url": url, "api_key": secret,
                                                "model": "blackbox-owned", "dimensions": 768})
        job_id = client.call("reindex_workspace")["job_id"]
        with app.job_worker("worker-target:shared") as worker:
            while True:
                app.remaining()
                require(worker.poll() is None and app.server.poll() is None, "search process exited")
                job = client.call("migration_job_status", {"job_id": job_id})
                require(job["status"] in ("queued", "running", "completed"), f"reindex failed: {job}")
                if job["status"] == "completed":
                    require(app.rest("GET", f"/api/migration-jobs/{job_id}", key=key) == job,
                            "completed reindex differs across transports")
                    break
                time.sleep(min(0.1, app.remaining()))
            hits = client.call("search_entities", {"query_text": "owned search query", "entity_type": "note", "limit": 1})
            rest = app.rest("GET", "/api/search?query_text=owned%20search%20query&entity_type=note&limit=1", key=key)
            require(hits == rest and len(hits) == 1 and hits[0]["entity"]["id"] == entity["id"],
                    "queued search did not return the owned entity across transports")
            MCP(app, app.key).call("get_entity", {"id": entity["id"]}, error=True)
        require(any("owned search document" in text for text in requests)
                and requests.count("owned search query") >= 2, "document or query bypassed the owned provider")
    writer.call("reset_embedding_provider")


def cross_tenant_rejection(app):
    account, key = human_owner(app)
    client = MCP(app, key)
    before = client.call("whoami")
    password = "blackbox-target-password"
    app.secrets.append(password)
    target_owner = app.rest("POST", "/auth/signup", {
        "email": "target@example.invalid", "password": password,
    }, status=201)
    target = target_owner["tenant_id"]
    target_login = app.login("target@example.invalid", password)
    workspace = app.create_workspace("inviting-workspace", issuer=target_login)
    _, target_key = app.issue(workspace, "schema", issuer=target_login)
    member = MCP(app, target_key)
    administrator = MCP(app, target_login)
    member.call("list_members", error=True)
    app.rest("GET", "/api/members", status=403, key=target_key)
    invitation = administrator.call("create_invite", {
        "email": "owner@example.invalid", "role": "admin", "expires_at": "2090-01-01T00:00:00Z",
    })
    token = invitation["token"]
    app.secrets.append(token)
    rejected = client.call("accept_invite", {"invite_token": token}, error=True)
    require("an account cannot belong to more than one tenant" in json.dumps(rejected),
            "cross-tenant acceptance did not report the membership conflict")
    app.rest("POST", "/auth/invites/accept-authenticated", {"invite_token": token}, status=409, key=key)
    require(client.call("whoami") == before and app.rest("GET", "/api/whoami", key=key) == before,
            "rejected acceptance retargeted the existing credential")
    identity = member.call("whoami")
    require(identity["tenant_id"] == target and identity["workspace_id"] == workspace,
            "target administrator selected the wrong workspace")
    members = administrator.call("list_members")
    require(members == app.rest("GET", "/api/members", key=target_login)
            and not any(row["user_id"] == account["user_id"] for row in members),
            "rejected membership exists or differs across transports")
    history = administrator.call("list_invites")
    require(any(row["email"] == "owner@example.invalid" and row["used_at"] is None for row in history)
            and token not in json.dumps(history), "rejected invitation was consumed or disclosed")


def scenarios(suite, edition="ce"):
    if edition == "ee":
        if suite == "tenant-postgres":
            return [("server.bootstrap", lambda app: app.boot()),
                    ("ee.authenticated-cross-tenant-rejection", cross_tenant_rejection)]
        require(suite == "mcp", "EE owns only its dedicated MCP delta suite")
        return [("server.bootstrap", lambda app: app.boot()),
                ("ee.discovery-auth-scope-input", mcp_suite),
                ("ee.column-worker-settings-lifecycle", enterprise_settings),
                ("ee.provider-settings-lifecycle", enterprise_providers),
                ("ee.schema-fork-upstream-merge", enterprise_schema_composition),
                ("ee.search-reindex-worker", enterprise_search)]
    common = [("server.bootstrap", lambda app: app.boot()),
              ("auth.rest-issue-rest-mcp-revoke", vertical_slice)]
    focused = {
        "rest": [("rest.schema-entity-crud", lambda app: crud(app, "rest")),
                  ("rest.relation-export-import-worker", lambda app: relation_export_import_worker(app, "rest")),
                  ("rest.builtin-template-relations", builtin_relations),
                 ("rest.auth-scope-input", rest_suite)],
        "mcp": [("mcp.schema-entity-crud", lambda app: crud(app, "mcp")),
                ("mcp.iam-policy-group-lifecycle", iam_lifecycle),
                ("mcp.relation-export-import-worker", lambda app: relation_export_import_worker(app, "mcp")),
                ("mcp.discovery-auth-scope-input", mcp_suite),
                ("mcp.migration-worker-undo", migration_lifecycle)],
        "cli": [("cli.arguments-tasks-stdout-effects", cli_suite),
                ("management.human-key-management", human_key_management),
                ("mcp.user-profile-lifecycle", user_profile_lifecycle),
                ("mcp.tenant-settings-lifecycle", tenant_settings_lifecycle),
                ("mcp.invite-lifecycle", invite_lifecycle),
                ("mcp.template-library-lifecycle", template_library_lifecycle),
                ("mcp.workspace-key-lifecycle", workspace_key_lifecycle),
                ("mcp.audit-history-lifecycle", audit_history_lifecycle),
                 ("mcp.membership-ownership-retirement", membership_retirement_lifecycle)],
    }
    return common + focused[suite]


def self_check():
    enterprise = scenarios("mcp", "ee")
    require(len({name for name, _ in enterprise}) == len(enterprise), "duplicate EE scenario IDs")
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
        print(json.dumps([name for name, _ in scenarios(args.suite, args.edition)]))
        return 0
    def interrupted(signum, _frame):
        raise InterruptedError(f"runner interrupted by signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    failure = None
    # Owned state is never placed in the artifact directory or a caller's configured database.
    with tempfile.TemporaryDirectory(prefix="yorishiro-blackbox-") as directory:
        app = App(args, directory)
        try:
            for result, (_, function) in zip(app.results, scenarios(args.suite, args.edition)):
                app.current_scenario = result["id"]
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
