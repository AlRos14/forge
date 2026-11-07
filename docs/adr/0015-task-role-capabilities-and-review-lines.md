# ADR 0015: TaskRole capabilities and review lines

Status: accepted for implementation
Date: 2026-11-06

## Context

Forge currently derives formal review and orchestration from the persisted
TaskRole name. A completed review WorkUnit also satisfies ordinary completion
dependencies, so a `request_changes` report can be mistaken for approval. These
rules make display names an authority source and cannot bind a correction to a
new approval of the corrected result.

## Decision

TaskRoles keep their existing IDs and names. A separate, versioned configuration
stores an optional display label, the closed capability set
`formal_review`, `orchestration`, and `request_rework`, and an optional bounded
rework route. Project defaults store role names and resolve them to Task-local
IDs when copied. Coordination and the existing strict orchestration policy
remain separate.

At admission, Forge records the exact TaskRole ID and version, configuration,
capabilities, and matching membership on the Execution. Provider Start checks
that the same TaskRole configuration and active membership are still current.
The snapshot is evidence for that attempt only. It grants no Workspace, Actor,
repository, or general Task authority. Later review acceptance checks the
admitted snapshot; a new mutation such as requesting rework also checks current
capability and current actor authority.

Formal review remains an ordinary allocated WorkUnit whose immutable subject
names one exact WorkUnit result. Review approval is represented by a durable
review line linking the source result, exact review reports, and corrections.
Only a favorable report over the latest result can satisfy the typed approval
requirement. A completion dependency alone never means approval.

Rework is manual unless an administrator persists an automatic route. The
route names Task-local destination and next-review roles, an explicit correction
Actor selection, and a distinct reviewer. One exact `request_changes` report
creates an immutable correction and dependent review pair atomically. Durable
request identity, content digest, and an eight-round Task budget make replay
safe. Missing eligibility, independence, or budget becomes a human-visible
block. Pause defers automatic materialization; cancellation prevents new work.

## Consequences

REST, MCP, generated TypeScript, defaults UI, review UI, and API documentation
share the Rust capability catalog and typed configuration. Unknown capability
values fail closed. Existing literal `reviewer` and `orchestrator` roles receive
the corresponding capabilities through a provenance-recorded additive
migration; other roles receive none. Historical records are not rewritten.

The current orchestrator action allowlist and limits do not change. Capabilities
do not create autonomous merge, assignment, or provider powers. CLI continues
to expose only operations it can prepare safely.
