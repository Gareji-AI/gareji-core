#!/usr/bin/env python3
"""Record one bounded Codex Stop event through the local Gareji Core bridge."""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import Any, Callable, Mapping, Sequence

MAX_INPUT_BYTES = 1_048_576
MAX_PROCESS_OUTPUT_BYTES = 1_048_576
MAX_CHANGED_PATHS = 200
MAX_PATH_CHARS = 512
CORE_BRIDGE_PROTOCOL_VERSION = "gareji.core-bridge.v0"
CHECKPOINT_SCHEMA_VERSION = "gareji.progress-checkpoint.v0"

Runner = Callable[..., subprocess.CompletedProcess[str]]


class HookError(Exception):
    """A bounded failure that is safe to surface in Codex."""


def resolve_core_binary(
    environ: Mapping[str, str],
    which: Callable[[str], str | None] = shutil.which,
) -> str:
    """Resolve explicit, PATH, then Gareji Setup's platform install location."""
    configured = environ.get("GAREJI_CORE_BIN")
    if configured:
        return configured
    on_path = which("gareji-core")
    if on_path:
        return on_path
    if sys.platform == "win32":
        base = environ.get("LOCALAPPDATA")
        candidate = Path(base) / "Gareji" / "bin" / "gareji-core.exe" if base else None
    elif sys.platform == "darwin":
        home = environ.get("HOME")
        candidate = (
            Path(home)
            / "Library"
            / "Application Support"
            / "Gareji"
            / "bin"
            / "gareji-core"
            if home
            else None
        )
    else:
        base = environ.get("XDG_DATA_HOME")
        if not base and environ.get("HOME"):
            base = str(Path(environ["HOME"]) / ".local" / "share")
        candidate = Path(base) / "Gareji" / "bin" / "gareji-core" if base else None
    if candidate is not None and candidate.is_file():
        return str(candidate)
    return "gareji-core"


def run_command(
    args: Sequence[str],
    *,
    runner: Runner,
    input_text: str | None = None,
    timeout: float = 5,
) -> subprocess.CompletedProcess[str]:
    try:
        completed = runner(
            list(args),
            input=input_text,
            capture_output=True,
            encoding="utf-8",
            check=False,
            timeout=timeout,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise HookError("a required local command is unavailable") from error
    if len(completed.stdout.encode("utf-8")) > MAX_PROCESS_OUTPUT_BYTES:
        raise HookError("a local command returned too much output")
    return completed


def load_registrations(core_binary: str, runner: Runner) -> list[dict[str, Any]]:
    completed = run_command(
        [core_binary, "project", "list"],
        runner=runner,
    )
    if completed.returncode != 0:
        raise HookError("Gareji Core could not list registered projects")
    try:
        registrations = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise HookError("Gareji Core returned invalid project data") from error
    if not isinstance(registrations, list):
        raise HookError("Gareji Core returned invalid project data")
    return [item for item in registrations if isinstance(item, dict)]


def canonical_path(value: str | Path) -> Path:
    expanded = Path(value).expanduser()
    if sys.platform == "win32":
        raw = str(expanded)
        if raw.startswith("\\\\?\\UNC\\"):
            expanded = Path("\\\\" + raw[8:])
        elif raw.startswith("\\\\?\\"):
            expanded = Path(raw[4:])
    return expanded.resolve(strict=False)


def contains_path(root: Path, candidate: Path) -> bool:
    try:
        candidate.relative_to(root)
    except ValueError:
        return False
    return True


def select_project(
    registrations: list[dict[str, Any]],
    cwd: Path,
    environ: Mapping[str, str],
) -> tuple[dict[str, Any], Path] | None:
    configured_project = environ.get("GAREJI_PROJECT_ID")
    configured_workspace_path = environ.get("GAREJI_EXECUTION_WORKSPACE_PATH")

    if configured_project:
        registration = next(
            (
                item
                for item in registrations
                if item.get("project_id") == configured_project
            ),
            None,
        )
        if registration is None:
            raise HookError("the configured Gareji project is not registered")
        workspace_path = canonical_path(configured_workspace_path or cwd)
        if not contains_path(workspace_path, cwd):
            raise HookError("the Codex working directory is outside the configured workspace")
        return registration, workspace_path

    candidates: list[tuple[int, dict[str, Any], Path]] = []
    for registration in registrations:
        workspace_value = registration.get("execution_workspace")
        if not isinstance(workspace_value, str) or not workspace_value:
            continue
        workspace_path = Path(workspace_value).expanduser()
        if not workspace_path.is_absolute():
            continue
        workspace_path = canonical_path(workspace_path)
        if contains_path(workspace_path, cwd):
            candidates.append((len(workspace_path.parts), registration, workspace_path))
    if not candidates:
        return None
    _, registration, workspace_path = max(candidates, key=lambda item: item[0])
    return registration, workspace_path


def parse_porcelain_paths(payload: str, repository_root: Path, workspace: Path) -> list[str]:
    entries = payload.split("\0")
    paths: set[str] = set()
    index = 0
    while index < len(entries):
        entry = entries[index]
        index += 1
        if len(entry) < 4:
            continue
        status = entry[:2]
        raw_path = entry[3:]
        if "R" in status or "C" in status:
            index += 1
        absolute = (repository_root / raw_path).resolve(strict=False)
        try:
            relative = absolute.relative_to(workspace)
        except ValueError:
            continue
        normalized = relative.as_posix()
        if (
            not normalized
            or normalized.startswith("../")
            or len(normalized) > MAX_PATH_CHARS
        ):
            continue
        paths.add(normalized)
    return sorted(paths)[:MAX_CHANGED_PATHS]


def collect_git_state(
    workspace: Path,
    runner: Runner,
) -> tuple[list[str], dict[str, Any] | None]:
    root_result = run_command(
        ["git", "-C", str(workspace), "rev-parse", "--show-toplevel"],
        runner=runner,
        timeout=3,
    )
    if root_result.returncode != 0:
        return [], None
    repository_root = canonical_path(root_result.stdout.strip())
    if not contains_path(repository_root, workspace):
        return [], None

    status_result = run_command(
        [
            "git",
            "-c",
            "status.relativePaths=false",
            "-C",
            str(repository_root),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
        runner=runner,
        timeout=5,
    )
    changed_paths = (
        parse_porcelain_paths(status_result.stdout, repository_root, workspace)
        if status_result.returncode == 0
        else []
    )
    head_result = run_command(
        ["git", "-C", str(repository_root), "rev-parse", "--verify", "HEAD"],
        runner=runner,
        timeout=3,
    )
    branch_result = run_command(
        ["git", "-C", str(repository_root), "branch", "--show-current"],
        runner=runner,
        timeout=3,
    )
    head = head_result.stdout.strip() if head_result.returncode == 0 else None
    branch = branch_result.stdout.strip() if branch_result.returncode == 0 else None
    return changed_paths, {
        "head": head or None,
        "branch": branch or None,
        "dirty": bool(changed_paths),
    }


def require_string(payload: Mapping[str, Any], field: str) -> str:
    value = payload.get(field)
    if not isinstance(value, str) or not value:
        raise HookError(f"Codex Stop input is missing {field}")
    return value


def build_checkpoint(
    payload: Mapping[str, Any],
    registration: Mapping[str, Any],
    changed_paths: list[str],
    git_state: dict[str, Any] | None,
    now: dt.datetime,
) -> dict[str, Any]:
    session_id = require_string(payload, "session_id")
    turn_id = require_string(payload, "turn_id")
    project_id = registration.get("project_id")
    workspace_id = registration.get("execution_workspace")
    if not isinstance(project_id, str) or not project_id:
        raise HookError("the Gareji project registration has no identity")
    if not isinstance(workspace_id, str) or not workspace_id:
        raise HookError("the Gareji project registration has no workspace identity")
    identity = hashlib.sha256(
        f"{session_id}\0{turn_id}\0{project_id}".encode("utf-8")
    ).hexdigest()[:40]
    count = len(changed_paths)
    summary = (
        f"Codex turn ended with {count} changed path{'s' if count != 1 else ''}."
        if count
        else "Codex turn ended with no uncommitted workspace changes."
    )
    return {
        "schema_version": CHECKPOINT_SCHEMA_VERSION,
        "checkpoint_id": f"codex-stop-{identity}",
        "recorded_at": now.astimezone(dt.timezone.utc)
        .isoformat()
        .replace("+00:00", "Z"),
        "project_id": project_id,
        "work_item_id": None,
        "execution_workspace_id": workspace_id,
        "source": "codex_stop_hook",
        "actor": {"type": "agent", "id": "codex"},
        "outcome": "progress" if count else "no_action",
        "summary": summary,
        "changed_paths": changed_paths,
        "git": git_state,
        "verification": [],
        "evidence_refs": [],
        "recommended_state": None,
    }


def record_checkpoint(
    core_binary: str,
    checkpoint: Mapping[str, Any],
    runner: Runner,
) -> None:
    checkpoint_id = str(checkpoint["checkpoint_id"])
    request = {
        "protocol_version": CORE_BRIDGE_PROTOCOL_VERSION,
        "request_id": f"stop-hook-{checkpoint_id}",
        "operation": "record_progress",
        "payload": {"checkpoint": checkpoint},
    }
    completed = run_command(
        [core_binary, "bridge"],
        runner=runner,
        input_text=json.dumps(request, separators=(",", ":")) + "\n",
        timeout=10,
    )
    if completed.returncode != 0:
        raise HookError("Gareji Core could not record progress")
    response_line = completed.stdout.splitlines()[0] if completed.stdout else ""
    try:
        response = json.loads(response_line)
    except json.JSONDecodeError as error:
        raise HookError("Gareji Core returned an invalid progress response") from error
    if not isinstance(response, dict) or response.get("status") != "ok":
        code = (
            response.get("error", {}).get("code")
            if isinstance(response, dict)
            and isinstance(response.get("error"), dict)
            else None
        )
        suffix = f" ({code})" if isinstance(code, str) else ""
        raise HookError(f"Gareji Core rejected progress{suffix}")


def handle_stop(
    payload: Mapping[str, Any],
    *,
    environ: Mapping[str, str] = os.environ,
    runner: Runner = subprocess.run,
    now: dt.datetime | None = None,
) -> dict[str, Any] | None:
    if payload.get("hook_event_name") != "Stop":
        return None
    cwd = canonical_path(require_string(payload, "cwd"))
    core_binary = resolve_core_binary(environ)
    registrations = load_registrations(core_binary, runner)
    selected = select_project(registrations, cwd, environ)
    if selected is None:
        return None
    registration, workspace = selected
    grants = registration.get("grants")
    if not isinstance(grants, list) or "write_progress" not in grants:
        raise HookError("the matched Gareji project does not grant progress writes")
    changed_paths, git_state = collect_git_state(workspace, runner)
    checkpoint = build_checkpoint(
        payload,
        registration,
        changed_paths,
        git_state,
        now or dt.datetime.now(dt.timezone.utc),
    )
    record_checkpoint(core_binary, checkpoint, runner)
    return None


def safe_hook_message(error: HookError) -> dict[str, Any]:
    return {
        "continue": True,
        "systemMessage": f"Gareji Progress: {error}",
    }


def main() -> int:
    data = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
    if len(data) > MAX_INPUT_BYTES:
        print(json.dumps(safe_hook_message(HookError("Stop input is too large"))))
        return 0
    try:
        payload = json.loads(data)
        if not isinstance(payload, dict):
            raise HookError("Codex Stop input must be a JSON object")
        output = handle_stop(payload)
    except json.JSONDecodeError:
        output = safe_hook_message(HookError("Codex Stop input is invalid JSON"))
    except HookError as error:
        output = safe_hook_message(error)
    except Exception:
        output = safe_hook_message(HookError("unexpected local capture failure"))
    if output is not None:
        print(json.dumps(output, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
