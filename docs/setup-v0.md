# First-run setup v0

Status: implemented by the standalone `gareji-bootstrap` crate and its `gareji` binary.

## Interface

```text
gareji setup --workspace <absolute-directory> --context <absolute-markdown> [--context ...]
gareji setup ... --dry-run
gareji doctor [--project-id <id>]
gareji status [--workspace <directory>] [--limit <1-100>]
```

Setup derives the project identity and display name from the workspace directory unless they are supplied explicitly. A non-ASCII-only directory name requires `--project-id`. Every workspace and context path must be absolute and exist, and every context file must have a `.md` extension.

`--dry-run` validates all local inputs through Core's non-mutating `project validate` command, verifies the Python runtime required by the Stop Hook, and reports the intended layers without copying Core or invoking mutating Core or Codex commands. `--json` returns the same bounded report for automation.

If the project identity already exists with different settings, Setup stops without replacing it. Review the existing registration and pass `--replace-project` only when the generated reference-only registration should replace it.

## Apply order

Before mutation, Setup validates the generated registration with the supplied Core binary and requires `python` on Windows or `python3` on Linux and macOS. One successful setup then performs these operations in order:

1. Hash and install the supplied or sibling gareji-core binary in the platform application-data `Gareji/bin` directory.
2. Create or replace one registration through `gareji-core project register`, then verify it through `project list`.
3. Add the distribution root through `codex plugin marketplace add` when `gareji-local` is not already configured.
4. Read the requested Plugin version from its manifest. Install it through `codex plugin add`, or refresh a stale or disabled installation through `codex plugin remove` followed by `codex plugin add`.
5. Return a restart and Hook-review instruction when the Codex installation changed.

The generated registration grants only `read_context` and `write_progress`. Each Markdown file is stored as an attributed `evidence_ref` with no inline content, and `delivery_targets` is empty.

## Idempotency and conflicts

Setup compares the installed binary hash, the complete project registration, the Marketplace name and canonical root, and the installed Plugin identity, version, and enablement. Matching state is left unchanged. A stale or disabled Gareji Progress Plugin is reinstalled from the configured local Marketplace. A differing project registration or a `gareji-local` Marketplace name mapped to another root fails rather than being overwritten; project replacement requires `--replace-project`.

Setup does not write a second state file. Core remains the registration and Progress Checkpoint authority, while Codex remains the Marketplace, Plugin, enablement, and Hook-trust authority.

## Distribution discovery

By default the `gareji` binary finds gareji-core beside itself and walks upward to find both `.agents/plugins/marketplace.json` and `plugins/gareji-progress`. Development builds satisfy this layout from the repository root. The release bundle preserves the same layout, so an extracted distribution needs no provider-specific configuration. Advanced layouts may pass `--core-source` and `--marketplace-root` explicitly. Setup and the Stop Hook share the machine-specific Core location through [Platform layout v0](platform-layout-v0.md).

`scripts/package_gareji.py` builds a versioned ZIP for Windows or `tar.gz` for Linux and macOS. It includes the two binaries, Marketplace, runtime Plugin files, setup documentation, a file manifest, and a SHA-256 sidecar. Tests and Python bytecode are excluded from the runtime Plugin copy.

With `--smoke-test`, packaging runs both binaries and executes a complete `gareji setup --dry-run` against a temporary workspace and absolute Markdown context path. The check uses an isolated install destination and does not modify Codex or Core state. CI runs this check for every supported target before uploading the archive.

The local Marketplace format and Codex CLI installation flow follow the current Codex plugin authoring model documented in [Build plugins](https://learn.chatgpt.com/docs/build-plugins.md).

## Diagnosis

`gareji doctor` performs no install, registration, or repair operation. It checks the installed Core binary, an optional project identity, the `gareji-local` Marketplace, the installed-and-enabled Gareji Progress Plugin, and the Python runtime used by its Stop Hook. It exits successfully only when every requested layer is ready.

`gareji status` is the project-centered daily Interface. It resolves the current or supplied workspace to one registration, reuses Doctor's non-repairing health checks, and reads a bounded newest-first page through `gareji-core progress list`. Its human view shows the project identity, context-reference count, integration health, checkpoint summaries, and at most five paths per checkpoint. Its JSON view returns the same bounded `StatusReport`. It does not dereference context or write Core, Codex, or project state.
