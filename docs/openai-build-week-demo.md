# OpenAI Build Week demo

This is the recording path for a public demo of Garage AI's first implementation, Gareji Core. Keep the finished video under three minutes and use a clean disposable Git repository with one Markdown context file and one or two source files.

## Story

The problem is not a lack of memory providers. Developers already have local Markdown, Obsidian-style vaults, GBrain, LLMWiki, and other choices. The problem is coupling an AI development workflow to one of them. Gareji keeps trust, execution, and progress stable while context and provider-specific write-back remain replaceable.

## Recording sequence

### 0:00-0:25 — Problem and promise

Show the clean demo repository and its `PROJECT.md`. Explain that Garage AI is building a dependable software development factory where memory layers and agent runtimes can change without redesigning the trusted core.

### 0:25-0:55 — One-command setup

Preview and apply setup:

```powershell
gareji setup --workspace $PWD.Path --context (Join-Path $PWD.Path "PROJECT.md") --dry-run
gareji setup --workspace $PWD.Path --context (Join-Path $PWD.Path "PROJECT.md")
```

Point out the four visible layers: Core, project registration, local Marketplace, and Codex Plugin. If installation changed, restart Codex and trust the Stop Hook before recording the remaining sequence.

### 0:55-1:50 — Real Codex work

Ask Codex with GPT-5.6 to make one small, visible, testable change using `PROJECT.md` as context. Show the change and its test. The narration must explain what Codex did and where GPT-5.6 contributed; do not show private prompts, credentials, or unrelated repository state.

### 1:50-2:30 — Progress becomes durable

End the Codex turn, then show the result through the product Interface:

```powershell
gareji status --limit 1
```

The screen should show the project and overall state first, all four integration layers as ready, one configured context reference, the latest activity summary, and only the demo's one or two changed paths. The narration should describe these as bounded project facts rather than as an AI persona or an "insight" layer.

### 2:30-2:55 — Why the design matters

Explain that the Stop Hook records bounded evidence locally, while provider-specific Adapters are added only for real authentication, translation, or write-back. Close on the ability to change memory layers without replacing Core policy or progress history.

## Before recording

- Use a disposable repository with no personal files or prior checkpoints.
- Keep the repository clean before the Codex turn.
- Increase terminal font size and use `gareji status --limit 1`.
- Confirm the public video contains no private repository URL, credentials, audit sidecars, or local personal paths.
- Record voiceover that explicitly covers the project, Codex, and GPT-5.6.
- Upload only after the user approves the final cut.
