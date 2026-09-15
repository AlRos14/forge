# ADR 0002: Human and Agent actor parity

Status: Accepted in PR 0

## Context

Treating Humans as special review callbacks prevents human planning,
implementation, and orchestration from using the same audit and authority
model as AI work.

## Decision

Human and Agent are peer Actor kinds. A Human performs work through the same
Role, Execution, Artifact, Evidence, Gate, Workspace, and collaboration
primitives. Human UI ergonomics may differ.

## Consequences

No fake Agent or HarnessSession is needed for human work, and domain policy
does not grant or deny authority merely from Actor kind.

## Migration

PR 1 persists Human ActorRefs in all initial roles. PRs 7, 8, and 12 replace
special planner/reviewer human paths and UI states.
