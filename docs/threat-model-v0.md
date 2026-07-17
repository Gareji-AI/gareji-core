# Threat model v0

Status: **Accepted for Phase 0** (reconciled to merged implementation).

## Assets

- Host process integrity and deterministic failure classification
- Compact stdout integrity for machine consumers
- Audit sidecar completeness for local operators
- Secrets that may appear in input JSON, paths, plugin payloads, stderr

## Trust assumptions (Phase 0)

- Plugins are **trusted local absolute native executables** selected explicitly by the operator.
- Host does not sandbox, sign-verify, or discover plugins.
- Environment clearing + `--plugin-env KEY` allowlist reduce accidental credential inheritance; they are **not** a sandbox.
- Structured `--redact-pointer` rules reduce accidental exposure; they are **not** a secret-free guarantee.
- Every sidecar is **potentially sensitive**.

## Explicitly unsupported

- Relative paths / PATH lookup / shell wrappers / shebang scripts
- Remote plugins, registries, signing, sandboxing
- Process-tree / grandchild containment
- Exact deterministic replay (redaction removes values)
- Network / removable filesystems / unresolved Windows reparse points as sidecar targets
- Elevated/admin execution

## Controls present in Phase 0

Verified against the Phase 0 host/CLI:

- Direct process spawn (`shell=false`)
- Absolute native executable path checks before spawn
- Bounded headers/frames/JSON trees/stderr/sidecars/timeouts
- Owner-only permissions where the OS supports them
- Write-once no-overwrite sidecar commit on local filesystems
- Plugin stderr hash/count-only by default; raw opt-in marks `sensitive_content=true`
- Host diagnostics never echo raw plugin stderr
- Redaction pointers validated before spawn; zero pointers recorded as explicit no-redaction policy

## Residual risks

- A trusted plugin can still read user-accessible files and act with inherited OS authority.
- Free-form stderr and unknown payload fields may contain secrets even when pointers are redacted.
- Crash durability of sidecar commits is not universally guaranteed; visible finals are complete JSON only.
- Because write-once forbids overwrite, post-commit metadata such as `commit.sync_level` may be absent from the visible final sidecar even after a successful persist.
- Quantitative coverage tooling and some named failure golden/system tests remain deferred hardening; residual risk is incomplete automated regression density, not an undocumented trust boundary.
