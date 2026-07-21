# Gareji CLI presentation language v0

Status: implemented for the human-facing `gareji` CLI.

## Purpose

Gareji is a local trust kernel, not an AI persona. Its user-facing Interfaces explain the bounded object being inspected, its current state, relevant exceptions or recent activity, and one concrete next action when action is required.

The CLI must remain useful without AI-specific decoration, terminal color, animation, or marketing language.

## Information order

Human-facing surfaces use this order:

1. The project or execution target.
2. The overall state.
3. System layers that need inspection.
4. Recent activity or evidence.
5. One concrete `Next:` instruction, only when useful.

Do not begin a general product surface with a chat prompt. Put AI-assisted actions beside the project object or state they affect.

## Vocabulary

Use the domain terms in [the Gareji Core context](../CONTEXT.md). In particular:

- Gareji Core is the local authority, not a Board, Runner, or scheduler.
- A Plugin is a trusted local executable, not a remote service.
- A Runner is a replaceable execution Adapter, not Core or a task-state authority.
- A Core Run is one bounded execution attempt, not a Work item or project.

Stable setup and diagnosis labels are `ready`, `changed`, `planned`, and `missing`. Overall human-facing state is `ready`, `changed`, `planned`, or `needs attention`. Never rely on color alone to distinguish these values.

## CLI presentation

The human view is plain text with deterministic ordering and spacing:

```text
<project name>
Project: <project id>
Workspace: <execution workspace>
Context references: <count>
Status: <overall state>

System
  <state>  <layer>  <bounded factual explanation>

Recent activity
  <time>  <outcome>
    <bounded factual summary>
    Changed
      <workspace-relative path>

Next: <one concrete action>
```

Normal output stays short. Problems use the order “condition, impact, next action.” Expected errors are written for a human; developer-only details belong behind explicit diagnostic or machine-readable Interfaces.

The human view must remain readable when stdout is redirected and without terminal color. `--json` remains the automation Interface and must not change as a side effect of presentation work.

## Review checklist

- [ ] The surface purpose can be explained without using the word AI.
- [ ] The target, current state, primary action, and latest relevant change are visible.
- [ ] Proposed, confirmed, and executed information are distinguishable.
- [ ] The output remains understandable without terminal color.
- [ ] Copy names an object, action, or observable result instead of using abstract promotion.
- [ ] CLI output remains readable when redirected, and JSON remains bounded and machine-readable.
- [ ] No credentials, personal paths, audit sidecars, or private payloads are exposed.

## Surface boundary

Gareji Core ships the human-facing `gareji` CLI and a bounded JSON Interface for automation. It does not ship a GUI or TUI. Gareji Board owns the human control surface; adding another Core surface requires a separate accepted design decision.
