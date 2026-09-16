# ADR 0009: Event-driven orchestrator wakeups

Status: Accepted in Plan PR0

## Context

Keeping an orchestrator model alive while workers run wastes resources and
encourages micromanagement.

## Decision

An orchestrator sleeps between meaningful durable events. Each wake is a new
orchestrate Execution and may reuse the task-scoped HarnessSession. Events are
debounced or coalesced when appropriate.

## Consequences

The system preserves durable cognition checkpoints without a continuously
generating supervisory loop. Wake causes and resulting actions are auditable.

## Migration

Plan PR6 adds the wake mechanism and multi-orchestrator policy. Plan PR9
removes workflow branches that attempt to encode cognitive decisions.
