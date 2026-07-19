from __future__ import annotations

import datetime as dt
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).parents[1] / "scripts" / "record_stop.py"
HOOKS = Path(__file__).parents[1] / "hooks" / "hooks.json"
SPEC = importlib.util.spec_from_file_location("record_stop", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
record_stop = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(record_stop)


class FakeRunner:
    def __init__(self, workspace: Path) -> None:
        self.workspace = workspace
        self.bridge_request = None

    def __call__(self, args, **kwargs):
        if args[-2:] == ["project", "list"]:
            output = json.dumps(
                [
                    {
                        "project_id": "core",
                        "name": "Core",
                        "execution_workspace": str(self.workspace),
                        "context_sources": [],
                        "grants": ["write_progress"],
                        "sourced_context": [],
                        "delivery_targets": [],
                    }
                ]
            )
            return completed(args, output)
        if args[-1:] == ["bridge"]:
            self.bridge_request = json.loads(kwargs["input"])
            output = json.dumps(
                {
                    "status": "ok",
                    "protocol_version": "gareji.core-bridge.v0",
                    "request_id": self.bridge_request["request_id"],
                    "result": {},
                }
            )
            return completed(args, output + "\n")
        if "rev-parse" in args and "--show-toplevel" in args:
            return completed(args, str(self.workspace) + "\n")
        if "status" in args:
            return completed(args, " M src/lib.rs\0?? notes/new.md\0")
        if "rev-parse" in args and "--verify" in args:
            return completed(args, "a" * 40 + "\n")
        if "branch" in args:
            return completed(args, "main\n")
        raise AssertionError(f"unexpected command: {args}")


def completed(args, stdout, returncode=0):
    return subprocess.CompletedProcess(args, returncode, stdout, "")


class RecordStopTests(unittest.TestCase):
    def test_hooks_file_contains_only_the_codex_hooks_root(self):
        hooks = json.loads(HOOKS.read_text(encoding="utf-8"))

        self.assertEqual(set(hooks), {"hooks"})
        windows_command = hooks["hooks"]["Stop"][0]["hooks"][0]["commandWindows"]
        self.assertIn("$env:PLUGIN_ROOT", windows_command)
        self.assertNotIn("%PLUGIN_ROOT%", windows_command)

    def test_local_command_output_is_decoded_as_utf8_on_windows(self):
        def runner(args, **kwargs):
            self.assertEqual(kwargs["encoding"], "utf-8")
            self.assertNotIn("text", kwargs)
            return completed(args, "日本語のパス\n")

        result = record_stop.run_command(["gareji-core", "project", "list"], runner=runner)

        self.assertEqual(result.stdout, "日本語のパス\n")

    def test_resolves_the_setup_install_location_after_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            if sys.platform == "win32":
                core = root / "Gareji" / "bin" / "gareji-core.exe"
                environ = {"LOCALAPPDATA": str(root)}
            elif sys.platform == "darwin":
                core = (
                    root
                    / "Library"
                    / "Application Support"
                    / "Gareji"
                    / "bin"
                    / "gareji-core"
                )
                environ = {"HOME": str(root)}
            else:
                core = root / "Gareji" / "bin" / "gareji-core"
                environ = {"XDG_DATA_HOME": str(root)}
            core.parent.mkdir(parents=True)
            core.write_bytes(b"core")

            resolved = record_stop.resolve_core_binary(environ, which=lambda _: None)

            self.assertEqual(resolved, str(core))

    def test_explicit_core_binary_wins_over_path_and_setup_location(self):
        resolved = record_stop.resolve_core_binary(
            {"GAREJI_CORE_BIN": "C:/explicit/gareji-core.exe"},
            which=lambda _: "C:/path/gareji-core.exe",
        )

        self.assertEqual(resolved, "C:/explicit/gareji-core.exe")

    def test_records_one_bounded_checkpoint_for_the_matching_workspace(self):
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary).resolve()
            runner = FakeRunner(workspace)
            payload = {
                "hook_event_name": "Stop",
                "session_id": "session-1",
                "turn_id": "turn-1",
                "cwd": str(workspace / "src"),
                "model": "gpt",
            }

            output = record_stop.handle_stop(
                payload,
                environ={},
                runner=runner,
                now=dt.datetime(2026, 7, 18, tzinfo=dt.timezone.utc),
            )

            self.assertIsNone(output)
            checkpoint = runner.bridge_request["payload"]["checkpoint"]
            self.assertEqual(checkpoint["project_id"], "core")
            self.assertEqual(
                checkpoint["changed_paths"], ["notes/new.md", "src/lib.rs"]
            )
            self.assertEqual(checkpoint["source"], "codex_stop_hook")
            self.assertEqual(checkpoint["outcome"], "progress")
            self.assertNotIn("transcript_path", checkpoint)

    def test_same_turn_produces_the_same_checkpoint_identity(self):
        payload = {
            "session_id": "session-1",
            "turn_id": "turn-1",
        }
        registration = {
            "project_id": "core",
            "execution_workspace": "workspace",
        }
        now = dt.datetime(2026, 7, 18, tzinfo=dt.timezone.utc)

        first = record_stop.build_checkpoint(payload, registration, [], None, now)
        second = record_stop.build_checkpoint(payload, registration, [], None, now)

        self.assertEqual(first["checkpoint_id"], second["checkpoint_id"])
        self.assertEqual(first["outcome"], "no_action")

    def test_ignores_workspaces_without_a_registration(self):
        with tempfile.TemporaryDirectory() as temporary:
            cwd = Path(temporary).resolve()
            self.assertIsNone(record_stop.select_project([], cwd, {}))

    def test_explicit_identity_supports_non_path_workspace_ids(self):
        with tempfile.TemporaryDirectory() as temporary:
            cwd = Path(temporary).resolve()
            registrations = [
                {
                    "project_id": "core",
                    "execution_workspace": "core-local",
                }
            ]
            registration, workspace = record_stop.select_project(
                registrations,
                cwd,
                {
                    "GAREJI_PROJECT_ID": "core",
                    "GAREJI_EXECUTION_WORKSPACE_PATH": str(cwd),
                },
            )

            self.assertEqual(registration["execution_workspace"], "core-local")
            self.assertEqual(workspace, cwd)

    @unittest.skipUnless(sys.platform == "win32", "Windows path representation")
    def test_matches_a_windows_verbatim_workspace_to_a_normal_cwd(self):
        with tempfile.TemporaryDirectory() as temporary:
            cwd = Path(temporary).resolve()
            registrations = [
                {
                    "project_id": "core",
                    "execution_workspace": "\\\\?\\" + str(cwd),
                }
            ]

            registration, workspace = record_stop.select_project(
                registrations, cwd, {}
            )

            self.assertEqual(registration["project_id"], "core")
            self.assertEqual(workspace, cwd)

    def test_failure_output_never_stops_codex(self):
        output = record_stop.safe_hook_message(
            record_stop.HookError("Core is unavailable")
        )
        self.assertTrue(output["continue"])
        self.assertEqual(
            output["systemMessage"], "Gareji Progress: Core is unavailable"
        )


if __name__ == "__main__":
    unittest.main()
