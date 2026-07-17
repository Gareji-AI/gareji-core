# Protocol v0

Status: **Accepted for the initial Gareji Core implementation.**
This is an early 0.x contract. It is **not** protocol 1.0 and may change before a public compatibility policy is adopted.

Wire version: `2026-07-15`
Host SemVer (Phase 0): `0.1.0`

## Transport

- JSON-RPC 2.0 over stdio
- LSP-style framing: `Content-Length: <bytes>\r\n\r\n` + UTF-8 JSON body
- Byte length (not character count)
- Header bounds: 8 KiB block, 1 KiB/line, 16 lines; exactly one `Content-Length`
- Unknown syntactically valid headers are ignored and recorded in the sidecar `unknown_headers`
- Duplicate object keys rejected before typed deserialization; batch arrays rejected
- Request IDs: non-empty visible ASCII, ≤128 bytes
- Host-owned envelopes reject unknown fields; opaque slots (`capabilities` values remain strings; plugin `input`/`payload`; application-error `data`) may contain plugin-owned JSON bounded by frame/tree limits

## Lifecycle

1. Host validates CLI input and redaction pointers (invalid pointers fail in the **input** category before spawn)
2. Host spawns an absolute native executable (`shell=false`); relative/PATH/shell/shebang rejected
3. Host sends `initialize` with supported protocol versions + host capabilities
4. Plugin returns selected protocol version, plugin semver, and capabilities
5. Host sends one `agentmesh.run` with host `run_id` + opaque `input`
6. Plugin returns result with opaque `payload` **or** a structurally valid JSON-RPC application error
7. Host closes stdin
8. During exit grace, host probes stdout to EOF; the first additional byte (including whitespace) is `unexpected_output`
9. Plugin must exit successfully within exit grace (no `shutdown` RPC in v0)
10. Host write-once commits the audit sidecar (when possible), prints exactly one compact JSON object to stdout, and exits

No notifications, concurrent requests, streaming, cancellation RPC, or host callbacks in v0.
Ctrl-C / host task cancellation maps to `host_interrupted` and attempts **direct-child** termination only.

## Methods and examples

### `initialize`

Request params:

```json
{
  "protocol_versions": ["2026-07-15"],
  "host_version": "0.1.0",
  "capabilities": ["compact_output", "sidecar_refs"]
}
```

Result:

```json
{
  "protocol_version": "2026-07-15",
  "plugin_version": "0.1.0",
  "capabilities": ["compact_output", "sidecar_refs"]
}
```

No shared protocol version ⇒ `protocol_version_mismatch` (no best-effort fallback).

### `agentmesh.run`

Request params:

```json
{
  "run_id": "252fb70a-805c-44da-9b86-1979161b1169",
  "input": { "hello": "agentmesh" }
}
```

Result:

```json
{
  "payload": { "echo": { "hello": "agentmesh" }, "run_id": "..." }
}
```

`input` / `payload` are plugin-owned opaque JSON. Host never promotes domain-specific fields into the envelope.

## Compact stdout

Exactly one JSON object (no trailing diagnostics on stdout):

```json
{
  "schema_version": "2026-07-15",
  "run_id": "...",
  "outcome": "ok",
  "payload": {},
  "artifacts": [".../full.json"],
  "diagnostics": []
}
```

Failure uses `outcome: "error"`, empty `payload` object, and `diagnostics[]` entries with `category`, `code`, and host-authored `message`.

Machine consumers rely on stdout + process exit code. Host diagnostics go to stderr and never echo raw plugin stderr.

## Audit sidecar

Default path:

```text
.agentmesh/runs/YYYY-MM-DD/<run-id>/full.json
```

Recorded fields (success example observed on local smoke):

- `protocol_version`, `host_version`, `plugin_version`, `run_id`
- `limits` (effective)
- `plugin_env_keys` (names only)
- `redaction` (`pointers`, `no_redaction_policy`, `redacted_field_count`)
- ordered `messages[]` (normalized/redacted values + `raw_sha256`)
- `unknown_headers`
- `stderr` (`byte_count`, `sha256`, `truncated`; optional raw only with `--capture-plugin-stderr` → `sensitive_content=true`)
- `timings_ms` (`initialize_ms`, `run_ms`, `close_ms`, `total_ms`, …)
- `exit_status`
- optional `primary_failure` / `secondary_failures[]`
- `hashes` (input / initialize_response / run_response; compact hash may be computed after emit)
- optional `interruption` (`host_interrupted`, `direct_child_termination_attempted`, `direct_child_exit_observed`)
- optional `commit` (`sync_level`, `commit_method`)

Write-once semantics: the host serializes, cap-checks, writes a same-directory temp file, syncs where supported, then renames with create-new/no-overwrite. Because the first commit must not be overwritten, **`commit` metadata may be absent from the visible final file** even when persistence succeeded (implementation records commit metadata after the first write and intentionally skips rewrite).

Sidecars are potentially sensitive local artifacts, not exact-replay logs (redaction replaces configured values).

## Limits (normative defaults)

| Limit | Default |
|---|---|
| input | 1 MiB |
| frame | 4 MiB |
| retained stderr | 256 KiB (then discard-drain) |
| initialize timeout | 5 s |
| run timeout | 60 s (configurable 1 s … 1 h) |
| exit grace | 2 s |
| sidecar | 10 MiB |
| JSON depth / nodes | 64 / 100_000 |
| header block / line / count | 8 KiB / 1 KiB / 16 |

## Exit categories

| Exit | Category |
|---:|---|
| 0 | success |
| 2 | input |
| 10 | protocol |
| 11 | plugin |
| 12 | timeout / cancelled |
| 13 | audit |
| 70 | internal |

Detailed codes include (non-exhaustive for operator diagnosis; stable categories are the 0.x compatibility surface):
`input_*`, `plugin_not_found`, `plugin_spawn_failed`, `plugin_write_failed`, `initialize_timeout`, `run_timeout`, `protocol_version_mismatch`, `invalid_framing`, `invalid_json`, `schema_violation`, `plugin_application_error`, `unexpected_eof`, `frame_too_large`, `rpc_id_mismatch`, `unexpected_output`, `plugin_exited`, `plugin_exit_timeout`, `host_interrupted`, `sidecar_too_large`, `sidecar_write_failed`, `stdout_write_failed`, `host_internal_error`.

Failure precedence: first terminal cause is `primary_failure`; later problems become ordered `secondary_failures[]`.

## Capabilities

Phase 0: `compact_output`, `sidecar_refs`. Unknown capability names are ignored but audited.

## JSON Schemas

Normative generated schemas: `schemas/protocol-v0.schema.json`
Drift is guarded by `agentmesh-proto` unit tests (`schema_snapshot_is_current`).

Covered envelopes include compact stdout plus `initialize` / `agentmesh.run` request and result shapes.

## Language neutrality

Protocol is language-neutral **by design**. Phase 0 proves Rust fixture separation only; polyglot evidence is deferred.

## Phase 0 evidence gate (accepted)

The initial implementation was accepted on the basis of:

- Green CI on `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, and `aarch64-apple-darwin`
- Immutable artifact manifest + downloaded CLI ↔ echo plugin smoke on each target
- Local `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`
- Local release CLI ↔ echo smoke producing compact `outcome=ok` and a bounded sidecar

Deferred hardening (not blockers for Phase 0 exit):

- Quantitative 100% branch-coverage tooling (tarpaulin / CI gate)
- Golden snapshots for every named failure envelope
- Dedicated `plugin_application_error` fixture + conformance assertion
- Conformance wiring for existing `exit-nonzero` fixture
- Platform Ctrl-C / host-interruption system tests
- Ensuring visible sidecar always includes `commit` metadata without violating no-overwrite
