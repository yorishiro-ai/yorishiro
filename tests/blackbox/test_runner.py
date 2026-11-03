"""Runner-only unit checks; these never launch the application."""

import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import run


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
                                          "YORISHIRO_LICENSE_KEY": "secret", "API_TOKEN": "secret"}):
                app = run.App(args, directory)
            self.assertNotIn("API_TOKEN", app.env)
            self.assertEqual(app.env["YORISHIRO_LICENSE_KEY"], "")
            self.assertEqual(app.env["YORISHIRO_EMBEDDING_PROVIDER"], "none")
            self.assertNotEqual(app.env["DATABASE_URL"], app.env["QUEUE_URL"])
            self.assertIn(str(Path(directory)), app.env["DATABASE_URL"])
            self.assertEqual(app.env["LOCO_CONFIG_FOLDER"], str(Path(directory) / "config"))

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

    def test_cleanup_escalates_and_reaps_timed_out_process(self):
        process = Mock(pid=123)
        process.poll.return_value = None
        process.wait.side_effect = [run.subprocess.TimeoutExpired("test", 5), 0]
        with patch("run.os.killpg") as kill:
            run.App.stop(process)
        self.assertEqual(kill.call_args_list[0].args, (123, run.signal.SIGTERM))
        self.assertEqual(kill.call_args_list[1].args, (123, run.signal.SIGKILL))
        self.assertEqual(process.wait.call_count, 2)


if __name__ == "__main__":
    unittest.main()
