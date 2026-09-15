# ADR 0006: WorkUnit is not plan truth

Status: Accepted in PR 0

## Context

Using WorkUnit as a normalized plan creates a second authority and forces
fragile bidirectional synchronization.

## Decision

WorkUnit is executable scope with dependencies and assignment context. A plan
is an Artifact. A WorkUnit may reference a plan without becoming its
representation.

## Consequences

Plans can coexist or evolve independently of historical WorkUnits. An
orchestrator can derive work without maintaining a plan state machine.

## Migration

PR 5 adds WorkUnits and a dependency DAG. PR 7 removes old plan authority
after plan Artifacts are authoritative.
