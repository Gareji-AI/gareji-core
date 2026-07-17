# Core bridge v0

`gareji-core bridge` is a local child-process Interface for trusted Gareji transports. It reads and writes one compact JSON object per line, bounds each request to 1 MiB, and keeps Registry and Progress Recorder storage inside Core.

The protocol discriminator is `gareji.core-bridge.v0`. Requests contain a caller-assigned `request_id` and exactly one of these operations:

- `list_projects`
- `get_project_context`
- `set_active_work_item`
- `record_progress`
- `get_checkpoint_status`
- `list_progress`

Responses repeat the protocol and request identities and contain either an operation result or a stable bounded error. Raw internal errors, credentials, transcripts, diffs, and SQLite details are never returned.

`list_progress` returns accepted checkpoints in newest-first intake order for one registered project and optional Work item. The caller supplies a limit from 1 through 100 and may continue with the returned checkpoint cursor. Each result includes the immutable checkpoint, current per-destination delivery state, attempt count, and the latest bounded delivery error when present. Reading requires the same `write_progress` project grant as recording and inspecting one checkpoint.

The first transport Adapter starts the Core binary as a child and keeps it alive. Closing the parent pipe ends the bridge process; v0 does not install or require a background daemon.

`set_active_work_item` also starts one `gareji-board bridge` child lazily and reuses it. Board must confirm the project relationship and active-work eligibility before Core changes its operational reference. `GAREJI_BOARD_BIN` and `GAREJI_BOARD_DB` may select explicit local Board installations; an unavailable Board fails closed without disabling unrelated Core operations.

## Project registration

Existing projects are attached explicitly from a reviewed JSON file:

```text
gareji-core --database ./gareji.sqlite3 project register --file ./project.json
gareji-core --database ./gareji.sqlite3 project list
```

See [`examples/project-registration-v0.json`](../examples/project-registration-v0.json). Registration is an idempotent create-or-replace operation. It does not mutate a repository, install Hooks, infer Work items, or publish anything.
