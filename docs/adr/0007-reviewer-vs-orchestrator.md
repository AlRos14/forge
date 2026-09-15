# ADR 0007: Reviewer and orchestrator are distinct responsibilities

Status: Accepted in PR 0

## Context

A continuously running reviewer cannot safely decide allocation, stopping,
reassignment, and formal correctness at once.

## Decision

A reviewer judges work and produces review output. An orchestrator directs work
and routes decisions. An Actor that needs both creates separate Executions
under separate Roles.

## Consequences

Review verdicts remain attributable and independent from supervisory actions.
Orchestrators cannot pass or fail implementation merely by observing it.

## Migration

PR 6 adds the orchestrator role. PR 8 replaces the special review runtime with
review Executions and generic Artifacts.
