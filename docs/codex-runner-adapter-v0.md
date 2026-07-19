# Codex Runner Adapter v0

Status: implemented in gareji-codex-runner.

## Interface

The Adapter satisfies gareji_core::runner::RunnerAdapter without adding Codex fields to Core. Its opaque request payload has three fields:

    {
      "prompt": "Implement the bounded task and verify it.",
      "workspace": "ABSOLUTE_PREPARED_WORKSPACE",
      "model": null
    }

The workspace must already exist and be absolute. Worktree creation, checkout, cleanup, Board state, and scheduling remain caller or Runner implementation responsibilities.

## Capability mapping

The v0 Adapter deliberately supports only two Core capabilities:

| Authorized capability | Codex sandbox |
|---|---|
| context.read | read-only |
| workspace.write | workspace-write |

Without workspace.write, the invocation remains read-only. Any other authorized capability returns a bounded unsupported_capability configuration failure without starting Codex. The Adapter never selects danger-full-access.

## Invocation

One request becomes one non-interactive invocation using:

    codex exec --json --ephemeral --sandbox <mapped-mode> --cd <workspace> -

The prompt is written through stdin rather than a command-line argument. The Adapter also:

- uses approval_policy="never" so an unattended Run fails instead of waiting for approval;
- ignores user configuration while retaining Codex authentication;
- disables lifecycle Hooks for the nested invocation;
- disables web search;
- clears configured MCP servers;
- disables terminal color;
- applies the Core-authorized execution deadline;
- launches without a visible console window on Windows.

GAREJI_CODEX_BIN may select an explicit Codex executable. Otherwise the Adapter resolves codex from PATH.

## Result translation

Codex stdout is parsed as bounded JSON Lines. The Adapter recognizes thread.started, completed agent-message items, turn.completed, turn.failed, and error. Unknown event types are ignored for forward compatibility, while malformed known events are protocol failures.

The compact success payload contains only:

- bounded thread identity;
- the bounded final agent message;
- known numeric token usage fields.

The raw JSONL stream, reasoning, command events, file-change events, stderr, environment, transcript, and credentials are never returned in the Core report. Stderr is drained with a fixed memory bound and discarded. An oversized stream, invalid UTF-8, invalid JSONL, missing terminal event, timeout, cancellation, spawn failure, and non-zero exit each map to stable bounded Runner failures.

## Deliberate omissions

The v0 Adapter is synchronous and in-process, matching the existing Runner seam. It does not expose a new Core bridge operation, persist Codex sessions, resume threads, prepare worktrees, invoke live web search, load MCP servers, or support production and destructive capabilities. Those additions require a concrete caller need and an explicit capability mapping.
