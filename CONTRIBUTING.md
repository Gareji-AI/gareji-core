# Contributing to Gareji Core

Gareji Core is currently developed privately while its publication rights and first public compatibility surface are prepared. These rules define the intended contribution standard; they do not authorize publication or external redistribution.

## Before changing code

1. Read [CONTEXT.md](CONTEXT.md), [the architecture](docs/architecture.md), and relevant ADRs.
2. Keep Board, runtime, tracker, and knowledge-provider fields out of Host envelopes.
3. Add behavior behind the narrowest existing Interface, or record an ADR before creating a new architectural seam.
4. Use synthetic fixtures only.

## Verification

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Commits

Use one independently reviewable and revertible meaning per commit. Keep its tests and schemas with the behavior they prove. Use an English Conventional Commit subject, for example `fix(host): preserve the first terminal failure`.

When useful, explain `Intent`, `Change`, `Verification`, and deliberate `Boundaries` in the message body. Never include confidential strategy, personal paths, credentials, real payloads, or sensitive Audit sidecars.
