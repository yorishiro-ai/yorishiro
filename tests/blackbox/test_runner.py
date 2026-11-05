"""Runner-only unit checks; these never launch the application."""

import contextlib
import io
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import run


class ToolInventoryTests(unittest.TestCase):
    def test_raw_export_records_success_without_parsing_ndjson_as_one_document(self):
        app = Mock(current_scenario="mcp.graph", tool_calls=[])
        client = run.MCP.__new__(run.MCP)
        client.app = app
        text = '{"kind":"schema"}\n{"kind":"entity"}\n'
        client.rpc = Mock(return_value={"result": {"content": [{"type": "text", "text": text}]}})
        self.assertEqual(client.call("export_jsonl", raw=True), text)
        self.assertEqual(app.tool_calls, [
            {"tool": "export_jsonl", "scenario": "mcp.graph", "outcome": "succeeded"},
        ])
        client.rpc.return_value = {"result": {"isError": True, "content": []}}
        with self.assertRaises(AssertionError):
            client.call("export_jsonl", raw=True)
        self.assertEqual(len(app.tool_calls), 1)

    def test_calls_record_only_successfully_validated_outcomes(self):
        app = Mock(current_scenario="mcp.example", tool_calls=[])
        client = run.MCP.__new__(run.MCP)
        client.app = app
        client.rpc = Mock(return_value={"result": {"content": [{"type": "text", "text": "{}"}]}})
        self.assertEqual(client.call("whoami"), {})
        client.rpc.return_value = {"result": {"isError": True, "content": []}}
        client.call("get_entity", error=True)
        with self.assertRaises(AssertionError):
            client.call("get_entity")
        self.assertEqual(app.tool_calls, [
            {"tool": "whoami", "scenario": "mcp.example", "outcome": "succeeded"},
            {"tool": "get_entity", "scenario": "mcp.example", "outcome": "rejected"},
        ])

    def test_reviewed_inventory_matches_source_tools(self):
        reviewed = json.loads((run.ROOT / "tests/blackbox/tools.json").read_text())
        for edition, directory in (("ce", "src/controllers/mcp"), ("ee_additions", "ee/controllers/mcp")):
            names = []
            for path in (run.ROOT / directory).glob("*.rs"):
                names.extend(re.findall(r'#\[tool\([\s\S]*?\)\]\s*pub(?:\(crate\))? async fn (\w+)', path.read_text()))
            self.assertEqual(sorted(names), reviewed[edition])
            self.assertEqual(len(names), len(set(names)))


class ProtocolTests(unittest.TestCase):
    def test_json_result_and_error(self):
        for payload in ({"result": {}}, {"error": {"code": -32602}}):
            document = {"jsonrpc": "2.0", "id": 7, **payload}
            self.assertEqual(run.rpc_document(json.dumps(document), 7), document)

    def test_sse_skips_notifications_and_other_ids(self):
        text = (': keepalive\r\n\r\n'
                'data:\r\n\r\n'
                'data: {"jsonrpc":"2.0","method":"notifications/message"}\r\n\r\n'
                'event: message\r\ndata: {"jsonrpc":"2.0","id":6,"result":{}}\r\n\r\n'
                'id: ignored\r\ndata: {"jsonrpc":"2.0",\r\n'
                'data: "id":7,"result":{"ok":true}}\r\n\r\n')
        self.assertEqual(run.rpc_document(text, 7)["result"], {"ok": True})

    def test_missing_duplicate_and_invalid_envelopes_fail(self):
        for text in ('{"jsonrpc":"2.0","id":8,"result":{}}',
                     '{"id":7,"result":{}}',
                     '{"jsonrpc":"2.0","id":7}',
                     '{"jsonrpc":"2.0","id":7,"error":{},"result":{}}',
                     'data: {"jsonrpc":"2.0","id":7,"result":{}}\n\n' * 2):
            with self.subTest(text=text), self.assertRaises(AssertionError):
                run.rpc_document(text, 7)

    def test_tool_json_and_error_shapes(self):
        success = {"result": {"content": [{"type": "text", "text": '[{"id":"one"}]'}]}}
        self.assertEqual(run.tool_value(success), [{"id": "one"}])
        for failure in ({"error": {"code": -32600}}, {"result": {"isError": True}}):
            self.assertTrue(run.tool_error(failure))
            with self.assertRaises(AssertionError):
                run.tool_value(failure)

    def test_export_tool_text_is_raw_jsonl_not_a_json_string(self):
        text = '{"kind":"schema","record":{"id":"one"}}\n{"kind":"entity","record":{}}\n'
        document = {"result": {"content": [{"type": "text", "text": text}]}}
        self.assertEqual(run.tool_text(document), text)
        api = run.GraphAPI.__new__(run.GraphAPI)
        api.surface = "mcp"
        api.client = Mock()
        api.client.rpc.return_value = document
        self.assertEqual(api.export(), text)
        api.client.call.return_value = {"job_id": "job"}
        self.assertEqual(api.submit(text), {"job_id": "job"})
        api.client.call.assert_called_once_with("import_jsonl", {"jsonl": text})


class TemplateFixtureTests(unittest.TestCase):
    def test_required_data_preserves_template_constraints_and_omits_optional_fields(self):
        fields = {name: {"required": True, **field} for name, field in {
            "choice": {"type": "string", "enum": ["first", "second"]},
            "date": {"type": "string", "format": "date"},
            "pattern": {"type": "string", "pattern": "[0-9X-]+"},
            "text": {"type": "string"}, "count": {"type": "integer"},
            "ratio": {"type": "number"}, "enabled": {"type": "boolean"},
            "items": {"type": "array"}, "object": {"type": "object"},
        }.items()}
        fields["optional"] = {"type": "string"}
        self.assertEqual(run.template_required_data(fields), {
            "choice": "first", "date": "2026-01-01", "pattern": "0000-0000-0000-000X",
            "text": "test", "count": 1, "ratio": 1, "enabled": True, "items": [], "object": {},
        })


class RedactionTests(unittest.TestCase):
    def test_known_unknown_and_bearer_secrets(self):
        text = 'secret-value ysr_prefix_private "Authorization": "Bearer arbitrary-token"'
        result = run.redact(text, ["secret-value", ""])
        for secret in ("secret-value", "ysr_prefix_private", "arbitrary-token"):
            self.assertNotIn(secret, result)
        self.assertIn('"Authorization": "Bearer [REDACTED]"', result)

    def test_serialized_cli_and_server_outputs(self):
        text = json.dumps({"stdout": "api key (shown once): ysr_ABC_DEF\n",
                           "stderr": "prefix=ysr_ABC"})
        result = run.redact(text)
        self.assertNotIn("ysr_", result)
        self.assertEqual(json.loads(result)["stderr"], "prefix=[REDACTED]")

    def test_nonsecret_text_unchanged(self):
        self.assertEqual(run.redact("HTTP 401 schema not found"), "HTTP 401 schema not found")

    def test_invite_stdout_redacted_before_parsing_registers_secret(self):
        for text in ("invite token (shown once): opaque-token\n", json.dumps({"stdout": "invite token (shown once): opaque-token\n"})):
            self.assertNotIn("opaque-token", run.redact(text))


class ArgumentTests(unittest.TestCase):
    def arguments(self, binary=None):
        return ["--binary", binary or sys.executable, "--suite", "mcp",
                "--edition", "ee", "--artifacts", "artifacts"]

    def test_valid_arguments(self):
        args = run.parse_args(self.arguments())
        self.assertEqual(args.suite, "mcp")
        self.assertEqual(args.edition, "ee")
        self.assertTrue(args.artifacts.is_absolute())

    def test_discovery_requires_no_binary(self):
        args = run.parse_args(["--suite", "rest", "--edition", "ce", "--list"])
        self.assertTrue(args.list)
        self.assertIsNone(args.binary)
        with patch("subprocess.Popen", side_effect=AssertionError("must not launch")):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(run.main(["--suite", "rest", "--edition", "ce", "--list"]), 0)
            ids = json.loads(output.getvalue())
            self.assertEqual(ids, [name for name, _ in run.scenarios("rest")])
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run.main(["--self-check"]), 0)

    def test_relative_missing_binary_and_unknown_suite_fail(self):
        cases = [self.arguments("relative"), self.arguments("/nonexistent/yorishiro"),
                 self.arguments() + ["--suite", "unknown"], self.arguments()[:-2]]
        for arguments in cases:
            with self.subTest(arguments=arguments), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as caught:
                    run.parse_args(arguments)
                self.assertEqual(caught.exception.code, 2)

    def test_stdout_fields_are_unique_and_uuids_validated(self):
        value = "00000000-0000-0000-0000-000000000001"
        self.assertEqual(run.output_field(f"noise\nworkspace id: {value}\n", "workspace id", True),
                         value)
        for text in ("", "workspace id: invalid", f"workspace id: {value}\nworkspace id: {value}"):
            with self.subTest(text=text), self.assertRaises((AssertionError, ValueError)):
                run.output_field(text, "workspace id", True)

    def test_child_environment_excludes_ambient_credentials_and_databases(self):
        args = run.parse_args(self.arguments())
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict("os.environ", {"DATABASE_URL": "unsafe", "QUEUE_URL": "unsafe",
                                          "YORISHIRO_LICENSE_KEY": "secret", "API_TOKEN": "secret",
                                          "LLVM_PROFILE_FILE": "/profiles/%m-%p.profraw"}):
                app = run.App(args, directory)
            self.assertNotIn("API_TOKEN", app.env)
            self.assertEqual(app.env["LLVM_PROFILE_FILE"], "/profiles/%m-%p.profraw")
            self.assertEqual(app.env["YORISHIRO_LICENSE_KEY"], "")
            self.assertEqual(app.env["YORISHIRO_EMBEDDING_PROVIDER"], "none")
            self.assertNotEqual(app.env["DATABASE_URL"], app.env["QUEUE_URL"])
            self.assertIn(str(Path(directory)), app.env["DATABASE_URL"])
            self.assertEqual(app.env["LOCO_CONFIG_FOLDER"], str(Path(directory) / "config"))

    def test_rest_extractor_rejection_is_text_not_json(self):
        app = Mock(key="credential", workspace="workspace")
        app.issue.return_value = ("id", "read-key")
        for status, text, passes in ((422, "missing field `schema_name`", True),
                                     (200, "missing field `schema_name`", False),
                                     (422, "unrelated rejection", False)):
            with self.subTest(status=status, text=text):
                app.http.side_effect = [(401, {}, "unauthenticated"), (status, {}, text)]
                if passes:
                    run.rest_suite(app)
                else:
                    with self.assertRaisesRegex(AssertionError, "extractor rejection"):
                        run.rest_suite(app)

    def test_task_name_does_not_conflict_with_name_parameter(self):
        args = run.parse_args(self.arguments())
        with tempfile.TemporaryDirectory() as directory:
            app = run.App(args, directory)
            with patch.object(app, "cli", return_value=("created", "")) as cli:
                self.assertEqual(app.task("create_tenant", name="blackbox"), "created")
                cli.assert_called_once_with("task", "create_tenant", "name:blackbox")
            app.close()

    def test_server_exit_fails_before_readiness_request(self):
        args = run.parse_args(self.arguments())
        with tempfile.TemporaryDirectory() as directory:
            app = run.App(args, directory)
            process = Mock()
            process.poll.return_value = 1
            value = "00000000-0000-0000-0000-000000000001"
            with patch.object(app, "cli"), patch.object(app, "task", side_effect=[f"tenant id: {value}", f"workspace id: {value}"]), \
                    patch.object(app, "issue", return_value=(value, "ysr_test")), \
                    patch("run.subprocess.Popen", return_value=process), patch.object(app, "http") as http:
                with self.assertRaisesRegex(AssertionError, "exited before readiness"):
                    app.boot()
                http.assert_not_called()
            app.close()

    def test_cli_boot_proves_timezone_preparation_before_migration(self):
        args = run.parse_args(self.arguments())
        args.suite = "cli"
        for diagnostic, accepted in (("no such table: timezone_state", True), ("unrelated error", False)):
            with self.subTest(diagnostic=diagnostic), tempfile.TemporaryDirectory() as directory:
                app = run.App(args, directory)
                with patch.object(app, "cli", side_effect=[("", diagnostic), RuntimeError("migration reached")]) as cli:
                    if accepted:
                        with self.assertRaisesRegex(RuntimeError, "migration reached"):
                            app.boot()
                        self.assertEqual(cli.call_args_list[1].args, ("db", "migrate"))
                    else:
                        with self.assertRaisesRegex(AssertionError, "timezone preparation"):
                            app.boot()
                        self.assertEqual(cli.call_count, 1)
                    self.assertEqual(cli.call_args_list[0].args, ("task", "create_tenant", "name:before-migrate"))
                    self.assertEqual(cli.call_args_list[0].kwargs, {"ok": False})
                app.close()

    def test_cleanup_escalates_and_reaps_timed_out_process(self):
        process = Mock(pid=123)
        process.poll.return_value = None
        process.wait.side_effect = [run.subprocess.TimeoutExpired("test", 5), 0]
        with patch("run.os.killpg") as kill:
            run.App.stop(process)
        self.assertEqual(kill.call_args_list[0].args, (123, run.signal.SIGTERM))
        self.assertEqual(kill.call_args_list[1].args, (123, run.signal.SIGKILL))
        self.assertEqual(process.wait.call_count, 2)

    def test_rest_import_sends_unquoted_raw_jsonl(self):
        args = run.parse_args(self.arguments())
        text = '{"kind":"schema","record":{}}\n{"kind":"entity","record":{}}\n'
        with tempfile.TemporaryDirectory() as directory:
            app = run.App(args, directory)
            response = Mock(status=202)
            response.getheaders.return_value = [("Content-Type", "application/json")]
            response.getheader.return_value = "application/json"
            response.read1.side_effect = [b'{"job_id":"job"}', b""]
            connection = Mock()
            connection.getresponse.return_value = response
            api = run.GraphAPI.__new__(run.GraphAPI)
            api.app, api.surface, api.key = app, "rest", "credential"
            with patch("run.http.client.HTTPConnection", return_value=connection):
                self.assertEqual(api.submit(text), {"job_id": "job"})
            request = connection.request.call_args.args
            self.assertEqual(request[:3], ("POST", "/api/import.jsonl", text))
            self.assertEqual(request[3]["Content-Type"], "application/x-ndjson")
            self.assertEqual(app.events[-1]["request"], text)
            connection.close.assert_called_once()
            app.close()

    def test_trickled_partial_body_cannot_extend_response_deadline(self):
        args = run.parse_args(self.arguments())
        for content_type in ("text/event-stream", "application/json"):
            with self.subTest(content_type=content_type), tempfile.TemporaryDirectory() as directory:
                app = run.App(args, directory)
                clock = [run.time.monotonic()]
                response = Mock(status=200)
                response.getheaders.return_value = [("Content-Type", content_type)]
                response.getheader.return_value = content_type

                def trickle(_size, clock=clock):
                    clock[0] += 0.4
                    return b'data: {"partial"'  # No newline or EOF.

                response.read1.side_effect = trickle
                connection = Mock()
                connection.getresponse.return_value = response
                with patch("run.http.client.HTTPConnection", return_value=connection), \
                        patch("run.time.monotonic", side_effect=lambda clock=clock: clock[0]), \
                        self.assertRaisesRegex(AssertionError, "response deadline exceeded"):
                    app.http("POST", "/mcp", timeout=1, rpc_id=7)
                self.assertEqual(response.read1.call_count, 3)
                timeouts = [call.args[0] for call in connection.sock.settimeout.call_args_list]
                self.assertEqual(len(timeouts), 3)
                self.assertGreater(timeouts[0], timeouts[1])
                self.assertGreater(timeouts[1], timeouts[2])
                connection.close.assert_called_once()
                app.close()

    def test_chunked_sse_preserves_priming_events_and_strict_json(self):
        args = run.parse_args(self.arguments())
        cases = [([b'data:\r\n\r\ndata: {"jsonrpc":"2.0","id":',
                   b'7,"result":{}}\r', b'\n\r\ndata: {incomplete'], None),
                 ([b'data: {broken}\n\n'], json.JSONDecodeError)]
        for chunks, error in cases:
            with self.subTest(chunks=chunks), tempfile.TemporaryDirectory() as directory:
                app = run.App(args, directory)
                response = Mock(status=200)
                response.getheaders.return_value = [("Content-Type", "text/event-stream")]
                response.getheader.return_value = "text/event-stream"
                response.read1.side_effect = chunks
                connection = Mock()
                connection.getresponse.return_value = response
                with patch("run.http.client.HTTPConnection", return_value=connection):
                    if error:
                        with self.assertRaises(error):
                            app.http("POST", "/mcp", rpc_id=7)
                    else:
                        _, _, text = app.http("POST", "/mcp", rpc_id=7)
                        self.assertEqual(run.rpc_document(text, 7)["result"], {})
                self.assertEqual(response.read1.call_count, len(chunks))
                connection.close.assert_called_once()
                app.close()

    def test_worker_timeout_stops_escalates_reaps_and_restores_deadline(self):
        args = run.parse_args(self.arguments())
        with tempfile.TemporaryDirectory() as directory:
            app = run.App(args, directory)
            deadline = app.deadline
            worker = Mock(pid=123)
            worker.poll.return_value = None
            worker.wait.side_effect = [run.subprocess.TimeoutExpired("worker", 5), 0]
            with patch("run.subprocess.Popen", return_value=worker) as popen, \
                    patch("run.os.killpg") as kill, \
                    self.assertRaisesRegex(AssertionError, "deadline exceeded"), app.job_worker() as process:
                self.assertIs(process, worker)
                self.assertLessEqual(app.deadline, deadline)
                app.deadline = 0
                app.remaining()
            self.assertEqual(popen.call_args.args[0], [str(args.binary), "--environment", "test_sqlite",
                                                      "start", "--worker=job-target:shared", "--no-banner"])
            self.assertIs(popen.call_args.kwargs["env"], app.env)
            self.assertTrue(popen.call_args.kwargs["start_new_session"])
            self.assertEqual([call.args for call in kill.call_args_list],
                             [(123, run.signal.SIGTERM), (123, run.signal.SIGKILL)])
            self.assertEqual(worker.wait.call_count, 2)
            self.assertIsNone(app.worker)
            self.assertEqual(app.deadline, deadline)
            app.close()

    def test_worker_failure_reaps_already_exited_child(self):
        args = run.parse_args(self.arguments())
        with tempfile.TemporaryDirectory() as directory:
            app = run.App(args, directory)
            worker = Mock()
            worker.poll.return_value = 1
            with patch("run.subprocess.Popen", return_value=worker), patch("run.os.killpg") as kill, \
                    self.assertRaisesRegex(AssertionError, "worker exited"), app.job_worker() as process:
                run.require(process.poll() is None, "worker exited")
            worker.wait.assert_called_once_with()
            kill.assert_not_called()
            app.close()

    def test_worker_cleanup_failure_still_stops_server_and_closes_logs(self):
        app = run.App.__new__(run.App)
        app.worker, app.server, app.logs = Mock(), Mock(), Mock()
        with patch.object(app, "stop", side_effect=[run.subprocess.TimeoutExpired("worker", 5), None]) as stop, \
                self.assertRaises(run.subprocess.TimeoutExpired):
            app.close()
        self.assertEqual([call.args for call in stop.call_args_list], [(app.worker,), (app.server,)])
        app.logs.close.assert_called_once()


if __name__ == "__main__":
    unittest.main()
