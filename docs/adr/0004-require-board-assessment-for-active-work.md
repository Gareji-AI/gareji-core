# ADR 0004: Require Board assessment for active work

- Status: **Accepted**
- Date: 2026-07-17
- Accepted-at: Board integration implementation review (2026-07-17)
- Deciders: Gareji Core maintainers

## Context

Core stores the operational active Work item reference used by MCP, Runner, CLI, and trusted lifecycle Hooks, but Board owns Work item existence, project relationship, and lifecycle state. Allowing Core to infer eligibility or read Board SQLite directly would duplicate state policy and couple separately released products.

## Decision

Core defines a `BoardPort` Interface with one operation: assess one explicit project and Work item pair for active work. The production Adapter keeps one `gareji-board bridge` child process and reuses it. Tests use an in-memory Adapter at the same Seam.

Core checks its local project grant first, then requires Board to report the Work item as eligible before changing the active-work reference. Board-not-found, Work-item-not-found, ineligible, and unavailable outcomes remain distinct bounded error categories. Core fails closed when Board is missing or returns an invalid response.

## Consequences

- Board remains the only interpreter of Work item state.
- Rejected assessments cannot overwrite a previously valid active-work reference.
- Other Core operations remain available when Board is not installed because the child starts lazily.
- A future Board daemon or Cloud coordination Adapter may replace the process Adapter without changing Core's Bridge Interface.
