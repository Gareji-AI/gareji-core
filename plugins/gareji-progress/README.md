# Gareji Progress Codex Plugin

This plugin bundles a non-blocking Codex Stop Hook that records one bounded Progress Checkpoint at the end of a turn.

The Hook does not parse the Codex transcript. It captures only the registered project and execution workspace identities, changed Git paths, HEAD, branch, and whether the workspace is dirty. Core remains the durable ledger owner.

## Prerequisites

- Run `gareji setup`, or place gareji-core on PATH, or set GAREJI_CORE_BIN.
- The project is registered in the same Core database used by the Hook.
- The registration grants write_progress.
- For automatic matching, execution_workspace is an absolute path containing the Codex working directory.

When execution_workspace is a stable identity rather than a path, configure:

- GAREJI_PROJECT_ID
- GAREJI_EXECUTION_WORKSPACE_PATH

GAREJI_CORE_DB selects a non-default Core database through the existing CLI environment option.

`gareji setup` installs Core under the platform application-data directory. The Hook discovers that location after checking GAREJI_CORE_BIN and PATH, so the default setup does not edit the user's PATH.

## Behavior

Unregistered directories are ignored. Capture errors return a Codex warning but always keep continue set to true, so progress recording cannot trap the agent in a Stop loop. The Checkpoint ID is deterministic for session, turn, and project, making repeated delivery idempotent.

After installing or changing the plugin, review and trust its Stop Hook with the Codex hook browser.
