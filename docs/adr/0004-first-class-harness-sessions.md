# ADR 0004: First-class HarnessSession

Status: Accepted in PR 0

## Context

Inferring continuity from a role name or the latest execution can resume the
wrong actor or harness and loses the external session's provenance.

## Decision

HarnessSession is a durable entity owned by an Agent and harness. Executions
attach to it explicitly. Parent execution lineage is separate from session
identity. Humans have no session.

## Consequences

Rework, restart, and recovery can target the exact native session or report
that it is unavailable or unsupported.

## Migration

PR 2 backfills only sessions that are sufficiently identifiable and marks
unknown historical semantics unknown. PR 13 removes legacy inference.
