# Review and validation

This page describes the current separation between formal review, deterministic validation, and Gate evaluation. The [task operating model](task-operating-model.md) and [delivery fluency](delivery-fluency.md) pages summarize how these records fit into Task delivery.


Validation and review are separate kinds of work and separate sources of
truth.

PR8 makes reviewer Executions and ReviewReport Artifacts authoritative for new
review results, and ValidationRuns/Evidence authoritative for deterministic
checks. V131 archives exact legacy Review and `review_evidence_bundle` rows in
`pr13_legacy_record_history` and drops their runtime tables. V099 provenance
ledgers remain detached migration history; none of these legacy rows satisfy a
PR9 Gate.

## Validation

Deterministic validation is represented by a core-controlled `ValidationRun`,
not by an Actor Execution. A ValidationRun does not require a Human, Agent,
HarnessSession, or fake System Actor. It records:

* the command or check identity;
* bounded environment and configuration summary;
* workspace and commit identity;
* start and finish times;
* status and exit code;
* a log or output reference.

The PR8 run produces Evidence and may produce a generic validation-report
Artifact through a dedicated ValidationRun producer relation.
Typical checks include
tests, typecheck, lint, build, security scanners, and required repository
commands. It is never attributed to a fake Actor or to an Actor Execution.

An Actor may perform validation-related cognitive work through ordinary
WorkUnit work: for example, reproducing a failure, investigating its cause, or
interpreting scanner output. That is distinct from the deterministic
ValidationRun and may itself produce an Artifact or Evidence.

### Catalog checks

A Project owner or admin may publish versioned **catalog checks**
(`validation_check_definition`): a slug `check_key`, a bounded shell command,
a timeout up to one hour and the `captured_default` environment policy. A
definition is immutable; publishing again creates the next version, retires the
previous active one and changes the `definition_digest`. Agents and the
coordinator never write the catalog.

A Task can use a catalog check only when its coordination envelope (schema
version 2) pins the exact `check_key` and `definition_digest` with a trigger:
`on_request` (the coordinator may ask for it) or `after_integration` (Forge runs
it after a successful WorkUnit integration). A later version never changes what
an existing pin authorizes: the pin turns stale (`validation.check_stale_pin`)
and must be re-authorized. A check without a pin is rejected with
`validation.check_not_authorized`.

Requests are durable `validation_check_request` rows consumed by the
`ValidationCheckRunner` inside the Forge process, not in a coordinator turn or
an Agent daemon. The runner resolves the exact subject (an accepted WorkUnit
result or the Task integration Workspace), defers while a worker lease is
active, takes the Workspace execution lock, and runs the pinned command through
the same core as legacy checks. The resulting `ValidationRun` records
`run_origin`, `catalog_check_id`, `definition_digest` and the request, with
identity `catalog:{check_key}@{definition_digest[0..16]}`. If the Workspace
changes after the request was queued, the request fails with
`validation.subject_changed` instead of validating a different subject. A
restart recovers requests left `running`.

A coordinator may request only a check key and definition digest pinned by the current Task authorization; it cannot supply a command, path, or timeout. Failed, error, or stale runs, and regression or inconclusive baseline comparisons, can create a source-bound validation_findings request. The coordinator can request a correction, recheck the same pinned subject, or raise a Human decision. These paths use the same durable budget and replay rules as other coordination requests. Structured Human decisions bind the exact source and evidence, require the expected version to answer, and resume only the matching authorized request.

With `baseline=true` the runner also runs the same command in a runner-owned,
disposable, detached git worktree at the exact base commit (no Workspace row,
removed afterwards) and stores an immutable `validation_comparison`. Rust, not
the model, classifies it: `regression`, `pre_existing_failure`, `fixed`,
`both_pass` or `inconclusive`. A `pre_existing_failure` does not satisfy a PASS
requirement.

Gate policy schema version 2 adds `required_checks`
(`{check_key, definition_digest, required_outcome: passed}`) for
merge-readiness Gates. Each is resolved by the exact merge candidate (the
integration Workspace commit and snapshot) and the pinned catalog identity, never
by "latest"; no matching run is missing evidence and more than one matching run
is an ambiguity. A `pre_existing_failure` is dispensed only by an exact approved
Decision whose `policy_ref` is `validation_waiver:{check_key}@{digest[0..16]}`.
Version 1 policies are unchanged and cannot carry `required_checks`; an unknown
schema fails closed.

Passing validation does not imply review passed. Validation may run without a
review and review may be required even when no validation command exists.

## Review

Review is an Execution admitted under a TaskRole with `formal_review`. Human and Agent reviewers use
the same domain record. The review Artifact records criteria, findings,
severity where useful, verdict, questions, references, the Evidence
considered, and the exact subject: Task, Workspace, base/head commit, snapshot
digest, and, when the review targets a WorkUnit result, the reviewed WorkUnit
version and result Execution/Artifact identity.

Prepare the initial reviewer WorkUnit with a `review_subject` naming the source
WorkUnit. While the source is open, its result version is unset. Accepting the
source freezes its admitted version, completed Execution, commit, Workspace,
and snapshot digest in the completion transaction. Later allocation or prompt
edits before acceptance therefore cannot leave the review bound to an obsolete
preparation version. A stale subject insertion racing completion is rejected.
The review becomes runnable only after the exact result is frozen.

Human reviewers submit through the reviewer WorkUnit's `review-report` route;
Forge creates the provenance attempt internally. Agent reviewers run through
ordinary WorkUnit dispatch. Both preserve contributor independence.

A review report that requests changes finishes its own review WorkUnit and is a
valid result, but it does not accept the reviewed implementation and does not
satisfy an approval Gate. A technical reviewer failure produces no valid
negative report.

A typed approval requirement follows a durable review line from its source
result through exact correction and review rounds. It is satisfied only by a
favorable report over the latest result on that line. A pass over an earlier
result and a completion dependency do not approve a later correction.

A reviewer may inspect changes, ask a question, request validation, pass, or
request changes. It does not directly mutate implementation work through a
review verdict.

PR9 Gate policies pin an exact ReviewReport ID/digest, its producing reviewer
Execution, verdict, subject, and permitted actor set. A Gate may also pin an
exact ValidationRun and Evidence pair. Neither input substitutes for the
other, and no reviewer verdict directly advances Task lifecycle.

## Multiple reviewers

Reviewer TaskRoles may be independent, partitioned, or collaborative:

* independent reviewers deliberately do not contaminate one another;
* partitioned reviewers cover distinct scopes;
* collaborative reviewers discuss before a Gate resolves.

A Gate can require all or a configured subset. One pass never hides another
reviewer's failure.

## Rework

The flow keeps request-changes in the exact ReviewReport and evaluates its Gate
as unsatisfied. A TaskRole needs `request_rework` and an explicit route before
it can request a correction. Manual mode is the default; automatic processing
requires an owner/admin policy. One idempotent domain operation over that exact report creates a
correction WorkUnit and a new reviewer WorkUnit, records their dependency,
provenance, and result subject, and consumes one bounded per-Task rework round
atomically. Replaying the same idempotency key consumes nothing and returns the
same units. The dispatcher performs the external starts; the rework operation
does not.

Opening a correction advances the line by the exact reviewed result. Another
reviewer's later report over that same result does not prevent a valid
request-changes report from opening the round. The new pending result clears
the line's previous approval while preserving every historical revision.

The previous implementation and review units are not reopened or rewritten.
A later review sees the correction result, not the earlier version. An
independent Human reviewer cannot have contributed to the reviewed result. An
Agent may reuse the same saved profile with a fresh Task/role-isolated native
session; contributor session history and forks cannot cross into review. Exhaustion, missing
assignment, unresolved disagreement, or expanded scope produces an explicit
human decision instead of an automatic approval. Rework receipts do not release
blocks owned by unrelated causes. Historical reviewer and implementer
Executions remain unchanged.

Quality requests are durable routing records, not ReviewReports. A
`delivery_correction` request names an exact ReviewReport source and the
responsible implementation route; it does not fabricate a reviewer verdict or
a ValidationRun. Review independence remains enforced by the existing review
line, Human contributor checks, and Agent role-session isolation.


## Related delivery concepts

See the [coordinator orchestration contract](orchestration.md), [Gate policy](gates.md), and [PR feedback loop](pr-feedback-loop.md).
