# ADR 0006: WorkUnit is not plan truth

Status: Accepted in Plan PR0

## Context

Using WorkUnit as a normalized plan creates a second authority and forces
fragile bidirectional synchronization.

## Decision

WorkUnit is executable scope with dependencies and allocation/assignment
context. RoleMembership remains only TaskRole participation and does not own
that scope. A plan is an Artifact. A WorkUnit may reference a plan without
becoming its representation.

## Consequences

Plans can coexist or evolve independently of historical WorkUnits. An
orchestrator can derive work without maintaining a plan state machine.

## Migration

Plan PR5 adds WorkUnits and a dependency DAG. Plan PR7 removes old plan
authority after plan Artifacts are authoritative.
