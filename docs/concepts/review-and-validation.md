# Review and validation

Validation and review are separate kinds of work and separate sources of
truth.

PR4 adds generic collaboration primitives but does not migrate the existing
Review, validation, `review_evidence_bundle`, or `task_decision_*` authorities.
Those continue to own their legacy data until PR8. PR4 Artifacts can represent
generic reports created through the new surface, but no legacy review or
validation writer automatically creates one.

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

The future PR8 run produces Evidence and may produce a generic validation-report
Artifact through a dedicated ValidationRun producer relation. PR4 supports
only Execution-produced Artifacts; it creates no ValidationRun foreign key.
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

## Multiple reviewers

Reviewer TaskRoles may be independent, partitioned, or collaborative:

* independent reviewers deliberately do not contaminate one another;
* partitioned reviewers cover distinct scopes;
* collaborative reviewers discuss before a Gate resolves.

A Gate can require all or a configured subset. One pass never hides another
reviewer's failure.

## Rework

Request-changes produces a ReviewReport Artifact and a rework Handoff. The
Handoff targets the selected implementer Actor and exact HarnessSession when
possible. It may also target a new Actor if orchestration or policy selects
one. Historical reviewer and implementer Executions remain unchanged.
