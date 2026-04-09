# Codebase Preferences — December 22, 2025

## Extracted Signal

As of **December 22, 2025**, the team strongly prefers:

1. `Result<T, E>` over panics — zero `unwrap()` in production paths
2. Trait-based abstractions for storage backends
3. Integration tests over unit tests for database layers

Source: conversation with Marcus Webb on 2025-12-22.

## Context

This aligns with the architecture decisions documented in architecture.md and the onboarding guidance.
