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
        self.worker = None
        self.server_files = []
        self.logs = contextlib.ExitStack()
        self.sequence = 0
        self.started = time.monotonic()
        self.deadline = self.started + 240
        self.results = [{"id": name, "status": "failed", "duration_seconds": 0,
                         "detail": "not executed"} for name, _ in scenarios(args.suite)]
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
        if self.args.suite == "cli":
            out, err = self.cli("task", "create_tenant", "name:before-migrate", ok=False)
            require("timezone_state" in (out + err), "unmigrated task must fail timezone preparation")
        self.cli("db", "migrate")
        tenant = output_field(self.task("create_tenant", name="blackbox"), "tenant id", True)
        self.tenant = tenant
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
                    return
            except (OSError, http.client.HTTPException):
                pass
            time.sleep(0.1)
        raise AssertionError("server readiness deadline exceeded")

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
                  "binary_sha256": digest.hexdigest(), "results": self.results}
        report_path = self.args.artifacts / "results.json"
        require(not report_path.is_symlink(), "report symlink refused")
        report_path.write_text(json.dumps(report, indent=2), encoding="utf-8")

    @contextlib.contextmanager
    def job_worker(self):
        require(self.worker is None, "worker already started")
        previous_deadline = self.deadline
        self.deadline = min(previous_deadline, time.monotonic() + 60)
        try:
            files = [self.logs.enter_context((self.directory / name).open("wb"))
                     for name in ("worker.stdout", "worker.stderr")]
            self.worker = subprocess.Popen(
                [str(self.args.binary), "--environment", "test_sqlite", "start",
                 "--worker=job-target:shared", "--no-banner"],
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
            return tool_text(self.client.rpc("tools/call", {"name": "export_jsonl", "arguments": {}}))
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
        value = output_field(app.task("create_workspace", tenant_id=app.tenant, name=name),
                             "workspace id", True)
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
    workspace = output_field(app.task("create_workspace", tenant_id=app.tenant, name="builtin-relations"),
                             "workspace id", True)
    _, schema_key = app.issue(workspace, "schema")
    _, write_key = app.issue(workspace, "write")
    templates = app.rest("GET", "/api/templates", key=schema_key)
    require(templates, "builtin template inventory is empty")
    for template in templates:
        definition = app.rest("GET", f"/api/templates/{template['id']}", key=schema_key)
        app.rest("POST", "/api/schemas", definition, status=201, key=schema_key)

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


def scenarios(suite):
    common = [("server.bootstrap", lambda app: app.boot()),
              ("auth.cli-issue-rest-mcp-revoke", vertical_slice)]
    focused = {
        "rest": [("rest.schema-entity-crud", lambda app: crud(app, "rest")),
                  ("rest.relation-export-import-worker", lambda app: relation_export_import_worker(app, "rest")),
                  ("rest.builtin-template-relations", builtin_relations),
                 ("rest.auth-scope-input", rest_suite)],
        "mcp": [("mcp.schema-entity-crud", lambda app: crud(app, "mcp")),
                ("mcp.relation-export-import-worker", lambda app: relation_export_import_worker(app, "mcp")),
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
