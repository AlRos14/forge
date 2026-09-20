# ADR 0011: Deterministic policy versus cognitive orchestration

Status: Accepted in Plan PR0

## Context

Models are useful for deciding whether to investigate, replan, reassign, or
ask a Human, but they cannot be authority for security, leases, credentials,
merge, or destructive policy.

## Decision

The core enforces deterministic authority, validation, leases, and Gates.
Actors and orchestrators provide cognitive direction through typed actions and
collaboration. The core does not grow an oversized cognition workflow DSL.

## Consequences

Policy outcomes are reproducible and fail closed while orchestration remains
flexible and harness-native. Unsupported actions are reported rather than
silently emulated.

## Migration

Plan PRs 3, 6, 8, and 9 move capability, orchestration, review, and lifecycle
decisions to the correct boundary.
