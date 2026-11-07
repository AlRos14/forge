# Review and validation

> Diseño objetivo aprobado, todavía pendiente de implementación:
> [modelo operativo de tareas y UX](task-operating-model.md). Define la
> separación Copilot/Orquestador, preparación desde objetivo y rutas de calidad.
> El resto de este documento describe el contrato actual salvo indicación expresa.


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
independent reviewer cannot be an Actor that contributed to the reviewed
result, even if it uses another Role or model label. Exhaustion, missing
assignment, unresolved disagreement, or expanded scope produces an explicit
human decision instead of an automatic approval. Rework receipts do not release
blocks owned by unrelated causes. Historical reviewer and implementer
Executions remain unchanged.

Quality requests are durable routing records, not ReviewReports. A
`delivery_correction` request names an exact ReviewReport source and the
responsible implementation route; it does not fabricate a reviewer verdict or
a ValidationRun. Review independence remains enforced by the existing review
line and the plan-level reviewer-Actor check.
