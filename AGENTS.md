# BrainJar

Rust-based personal knowledge management system with hybrid search (FTS, vector, graph, fuzzy), knowledge graph extraction, and temporal decay scoring.

## Build & Test

Before marking any work as complete, you MUST run:

```bash
cargo clippy --all-targets    # zero warnings required
cargo test                     # all tests must pass
```

Fix any warnings or failures before committing. Do not leave clippy warnings for someone else to clean up.

## Code Style

- Rust 2021 edition
- Follow existing patterns in the codebase
- Feature-gate test helpers that are only used behind feature flags (e.g. `#[cfg(feature = "golden-corpus")]`)
- Keep functions above `#[cfg(test)]` modules (clippy::items_after_test_module)
- No unused imports or dead code

## Git

- All agent commits and PRs go through the **farad-bots** GitHub identity
- Target the `dev` branch unless told otherwise
- Atomic commits with clear messages
