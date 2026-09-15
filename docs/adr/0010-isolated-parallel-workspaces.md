# ADR 0010: Isolated parallel implementation workspaces

Status: Accepted in PR 0

## Context

Concurrent writers in one working tree can overwrite, corrupt, or falsely
attribute changes.

## Decision

Each concurrent mutating WorkUnit/Execution receives an isolated branch or
worktree and explicit lease. Integration into the Task branch is a separate,
locked, deterministic operation.

## Consequences

Three implementers can work concurrently. Conflicts and integration failures
are visible outcomes rather than hidden Git accidents.

## Migration

PR 5 extends the existing workspace and git infrastructure. PR 15 proves
three-way concurrency, dependency blocking, integration, and conflict cases.
