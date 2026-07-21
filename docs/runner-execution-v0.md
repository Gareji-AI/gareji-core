# Runner execution v0

Status: implemented in `gareji-core::runner`.

## Interface

```text
RunExecutor::execute(core_run_request) -> rejected | finished | error
RunnerAdapter::execute(authorized_runner_request) -> runner_report
```

`CoreRunRequest` contains a stable Run identity, a positive execution deadline, requested capabilities, and one opaque JSON payload. Core validates the request, asks its trusted `ApprovalSource` for operation-bound approval evidence when required, and invokes the Runner Adapter only after the Capability Gate approves every requested capability.

The Adapter receives the Run identity, validated deadline, opaque payload, and the sorted set of capabilities Core authorized. `AuthorizedRunnerRequest` has no public constructor or public fields, so callers cannot turn an unapproved `CoreRunRequest` into the type accepted at the Runner seam. The Adapter does not receive an `approved=true` shortcut or raw approval records. Policy rejection is a normal `CoreRunResult::Rejected` result and never invokes the Adapter. Invalid input or unavailable approval state returns an error and also never invokes the Adapter.

## Bounded contract

- Run identities are non-empty, at most 128 characters, and contain no control characters.
- Execution deadlines are from 1 through 86,400 seconds.
- Request payloads are at most 1 MiB, 64 JSON levels, and 100,000 JSON nodes.
- Result payloads are at most 256 KiB with the same depth and node limits.
- Summaries are non-empty, at most 4,096 characters, and exclude unsafe control characters.
- A report carries at most 32 evidence references; kinds are stable lowercase identifiers and references are non-empty bounded text.
- Failed and cancelled reports require one bounded failure. Successful reports cannot carry a failure.
- Cancelled reports use the `cancelled` failure category; failed reports cannot use it.

Core rejects an invalid Adapter report instead of forwarding unbounded or internally inconsistent data. Raw transcripts, full diffs, credentials, process environment, and unbounded logs do not belong in either payload or evidence references.

## Ownership

Core owns request validation, capability enforcement, bounded report validation, and the stable Core Run result. A Runner Adapter owns one runtime invocation and translates provider events into the generic report.

Board owns scheduling, project and Work item state, Agent profiles, model selection, approvals, and reconciliation. The Codex Adapter owns Codex argument construction and event parsing. Workspace and Git worktree preparation belong to the caller or Runner implementation, not to the Core Interface.

The v0 seam is in-process and blocking. It does not add a daemon, remote Runner, Core bridge operation, or provider registry. Those require a concrete second transport or runtime and a separate accepted decision.

The first production Adapter is implemented in gareji-codex-runner. See [Codex Runner Adapter v0](codex-runner-adapter-v0.md).
