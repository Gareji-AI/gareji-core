# Progress Recorder v0

Status: implemented in `gareji-core::progress`.

## Interface

```text
record(checkpoint, delivery_targets) -> record receipt
status(checkpoint_id) -> checkpoint + per-destination delivery status
list(project, optional work item, cursor, limit) -> newest-first checkpoint page
sync_pending(projector) -> delivery summary
```

`record` validates the v0 Checkpoint bounds, requires relative changed paths without parent traversal, serializes the immutable payload, computes a SHA-256 fingerprint, and writes the checkpoint and initial delivery rows in one immediate SQLite transaction.

The first successful record captures both the immutable payload and its destination set. Retrying the same ID, payload, and destinations is a successful duplicate. Changing the payload produces a checkpoint conflict; changing the destination set produces a delivery-set mismatch. Adding a destination to historical checkpoints will use a future explicit replay operation rather than silently changing an idempotent record.

## Delivery

Every destination starts as `pending`. `sync_pending` attempts `pending` and `failed` deliveries through one `CheckpointProjector` router:

- `synced` is complete and is not retried;
- `failed` records a bounded sanitized message and is retried on the next explicit pass;
- `conflict` records a bounded sanitized message and requires review;
- success in one destination is never rolled back because another destination fails.

The projector must be idempotent by checkpoint ID. A process failure after external success but before the SQLite update can cause the same projection to be attempted again.

Projectors must return messages that are already safe to persist and display. The Recorder bounds their length and removes unsafe control characters; it cannot recognize or redact destination-specific secrets.

## Storage

`open_sqlite(path)` creates the parent application-data directory, enables foreign keys and WAL mode, applies a five-second busy timeout, and initializes the v0 tables. `open_in_memory()` runs the same implementation for conformance tests and disposable demos. Project and Work item scope are stored as indexed ledger columns so Board-facing history reads do not scan checkpoint JSON. Existing v1 databases are backfilled locally when first opened.

SQLite details remain inside the Module. Board, MCP, Runner, CLI, Hook, and Knowledge Adapters use the Recorder Interface and never issue its SQL directly.

The Core copy of `schemas/progress-checkpoint-v0.schema.json` is the canonical transport schema. Board mirrors that schema so its examples and UI-facing validation can run without importing Core internals; both copies must remain JSON-equivalent.

See [the Checkpoint example](../examples/progress-checkpoint-v0.json).
