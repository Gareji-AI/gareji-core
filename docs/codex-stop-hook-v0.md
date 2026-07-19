# Codex Stop Hook v0

Status: implemented by the gareji-progress Codex Plugin.

The plugin records a bounded Progress Checkpoint through the existing local Core bridge whenever a Codex turn stops in a registered execution workspace. It packages the Hook configuration and its cross-platform Python script under plugins/gareji-progress.

## Intake

The Hook uses stable Codex Stop fields: session ID, turn ID, event name, and working directory. It does not parse transcript_path because the transcript format is not a stable Hook Interface.

The Hook resolves the project in either of two ways:

1. Match the Codex working directory against an absolute execution_workspace in a Core project registration.
2. Use GAREJI_PROJECT_ID and GAREJI_EXECUTION_WORKSPACE_PATH when execution_workspace is a logical identity rather than a local path.

The Hook resolves Core in this order: `GAREJI_CORE_BIN`, `gareji-core` on PATH, then the platform application-data location written by `gareji setup`. A normal first-run setup therefore requires no persistent PATH or Hook environment modification.

Local command output is decoded as UTF-8 on every platform. On Windows, the Hook also treats normal drive paths and their `\\?\` verbatim equivalents as the same workspace, so Setup's canonical registration matches the working directory reported by Codex.

The Checkpoint contains relative changed Git paths, bounded HEAD and branch evidence, and no raw diff, transcript, prompt, credentials, or absolute local paths.

## Failure behavior

The Hook is observational and non-blocking. An unregistered directory is a silent no-op. A configured capture failure emits a bounded system warning with continue set to true. Core still validates the project grant, workspace identity, Checkpoint schema, and immutable identity before recording.

The Checkpoint ID is a SHA-256-derived identity over Codex session, turn, and project. Re-running the same Hook input therefore reaches the Recorder's normal idempotent path.

## Plugin lifecycle

Codex discovers the default hooks/hooks.json file when the plugin is enabled. Installed plugin Hooks must be reviewed and trusted before Codex runs them. Changes to the Hook definition require trust review again.
