# ADR 0003: Multi-actor Task roles

Status: Accepted in Plan PR0

## Context

One assignee per role prevents concurrent implementers, independent reviewers,
co-planners, and cooperating orchestrators.

## Decision

TaskRole owns a coordination mode and a set of RoleMembership records. An
Actor may belong to several roles on one Task. RoleMembership records
participation only; it has no WorkUnit, path, concrete scope, or current
assignment field. WorkUnit allocation and historical attempts belong to
WorkUnit and Execution. Execution records the role actually performed.

## Consequences

Assignment is a set and a historical relationship, not a singular field.
Policies remain explicit and small; role names do not instantiate behavior.

## Migration

Plan PR1 adds the membership schema and authoritative reads. Singular role
writers/readers remain only as bounded migration paths until Plan PR13.
