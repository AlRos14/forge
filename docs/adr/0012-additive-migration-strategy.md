# ADR 0012: Additive, reviewed migration

Status: Accepted in Plan PR0

## Context

The current repository has users, durable SQLite data, public surfaces, and
valuable workspace/daemon infrastructure. A big-bang rewrite would lose
history and make authority gaps unreviewable.

## Decision

Migrate in ordered Plan PRs: add replacement schema, write it, read it, stop old
writes, stop old reads, remove APIs/UI, then drop legacy schema. Every Plan PR
starts from current main, repeats dependency searches, documents invariants,
updates architecture docs, validates its touched paths, and stops.

## Consequences

Temporary compatibility is explicit and bounded. Historical uncertainty is
preserved. Destructive cleanup is delayed until migration fixtures prove that
user-owned data and audit history survive.

## Migration

The complete order and removal ledger are in
[architecture-v2.md](../migration/architecture-v2.md). Plan PR0 changes no
runtime or persistence.
