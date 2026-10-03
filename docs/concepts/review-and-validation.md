# Review and validation

Validation and review are separate kinds of work and separate sources of
truth.

PR8 makes reviewer Executions and ReviewReport Artifacts authoritative for new
review results, and ValidationRuns/Evidence authoritative for deterministic
checks. Legacy Review and `review_evidence_bundle` rows remain historical until
PR13; they do not satisfy a PR9 Gate.

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

An Actor may perform validation-related cognitive work through an ordinary
Execution with purpose `validate` or `investigate`: for example, reproducing a
failure, investigating its cause, or interpreting scanner output. That is
distinct from the deterministic ValidationRun and may itself produce an
Artifact or Evidence.

Passing validation does not imply review passed. Validation may run without a
review and review may be required even when no validation command exists.

## Review

Review is an Execution with purpose review under the reviewer Role. Human and
Agent reviewers use the same domain record. The review Artifact records
criteria, findings, severity where useful, verdict, questions, references, and
the Evidence considered.

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

The target flow keeps request-changes in the exact ReviewReport, evaluates the
Gate as unsatisfied, then lets lifecycle/orchestration policy consume that
failure and direct a Handoff or WorkUnit. The current PR9 branch implements the
first two facts only: it does not yet record retry-budget consumption or create
the rework Handoff/WorkUnit/Execution. The old workflow retry paths remain and
are a PR9 readiness blocker. Any later rework must carry exact failure identity
and must never infer a HarnessSession from a Role or latest Execution lookup.
Historical reviewer and implementer Executions remain unchanged.
