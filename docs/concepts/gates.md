# Gates

A Gate is a deterministic policy over durable facts. It does not contain model
cognition and never stores a mutable `passed` flag. A Gate has an immutable
identity and scope: Task, WorkUnit, exact TaskMerge operation, or exact Task
lifecycle transition. Each GatePolicyRevision is immutable, versioned,
schema-versioned, normalized, and digestible. Each GateEvaluation freezes one
policy revision, exact inputs, deterministic outcome, and result. Unknown
policy schemas and invalid digests fail closed.

## Policy inputs

Policy schema v1 accepts exact ReviewReport candidates, ValidationRun/Evidence
pairs, Proposal Decisions, WorkUnit requirements, and operation scope
requirements. A TaskMerge scope pins its operation ID, version, status, and the
exact merge-readiness GateEvaluation that admitted it. A lifecycle-operation
scope pins the exact immutable transition ID, states, versions, and cause.
TaskRole reviewer policies pin the role version and active membership set.

Every input records its Task, scope, version, digest, producer, subject, and
status where that fact provides them. A missing, stale, cross-Task, or
out-of-scope input cannot satisfy the Gate. Evaluation never resolves
`latest`. The same policy revision and normalized exact input set deduplicate
to one durable evaluation; a changed fact or policy produces a new historical
evaluation.

Validation requirements pin check identity, configuration digest, Workspace,
commit, snapshot digest, and required outcome. A CI PASS does not imply Review
PASS. A Review PASS does not imply that validation ran.

Proposal authorization reuses the exact Decision and its Proposal version,
outcome, policy reference/version/digest, and permitted ActorRefs. Login,
comments, Task transitions, and role membership alone are not approval.

WorkUnit requirements pin the exact WorkUnit version, dependency state,
Execution result, and, when required, exact WorkUnitIntegration. WorkUnit
completion is not integration.

## Review policy

The bounded reviewer policy supports one acceptable reviewer, all required
reviewers, or an N-of-M quorum. It may restrict ActorRefs, require a Human,
allow Humans or Agents, and pin a TaskRole membership snapshot. One Actor's
multiple reports count as one reviewer. A PASS never hides a required
request-changes or otherwise unacceptable report. The ReviewReport remains
cognitive truth; Gate only evaluates its exact provenance and verdict.

## Events, retries, and lifecycle

Durable domain events trigger evaluation after ReviewReport, ValidationRun,
Evidence, Decision, WorkUnit, WorkUnitIntegration, TaskRole, membership, policy,
or Task lifecycle-operation facts change. EventBus is only a wake hint. Event
claims, exact evaluation inputs, unique evaluation identity, and lifecycle
transition receipts make processing replay-safe. A source-event replay does
not recompute a previously committed evaluation using newer facts; it replays
that exact evaluation's effect.

Retry budgets are lifecycle/orchestration policy, never Gate inputs. A durable
receipt consumes each exact ReviewReport request-changes, failed ValidationRun,
failed Execution, or merge failure at most once. A rework receipt emits a
durable event for the Orchestrator; an exhausted budget blocks the Task and
prevents new Execution admission. Rework does not clear an unrelated block.

`TaskLifecycleService` owns aggregate Task progress and optimistic version
fencing. GateEvaluation may move an active Task to `ready_to_merge` only when
that exact current evaluation is satisfied. Merge admission then moves through
the existing serialized TaskIntegrationOperation and MergeService. Gate does
not perform review, validation, orchestration, or merge work. A current
unsatisfied GateEvaluation or verified exact retry receipt can direct a
merge-ready Task back to `active`; an Actor or legacy status transition cannot
clear merge readiness.

For merge readiness, ReviewReport and ValidationRun subjects must agree on
Workspace, commit, and snapshot. WorkUnit-backed merges require an exact
successful integration. Direct merge requires the completed source Execution
in the evaluated Workspace. MergeService holds the cross-process Task
integration lock while it checks the current evaluation and source commit.

REST/MCP/UI projection replacement belongs to PR12. Physical removal of
`task.status`, workflow tables, and historical transition data belongs to
PR13.
