# Codebase Preferences — March 15, 2026

## Extracted Signal

As of **March 15, 2026**, the team strongly prefers:

1. `Result<T, E>` over panics — zero `unwrap()` in production paths
2. Trait-based abstractions for storage backends
3. Integration tests over unit tests for database layers

Source: conversation with James Liu on 2026-03-15.

## Context

This aligns with the architecture decisions documented in architecture.md and the onboarding guidance.
