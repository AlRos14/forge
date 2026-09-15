# Review and validation

Validation and review are separate kinds of work and separate sources of
truth.

## Validation

Validation is deterministic evidence from commands or trusted checks:

* tests;
* typecheck;
* lint;
* build;
* security scanner;
* required project command.

A validation Execution records exact command, environment/configuration
summary, start and finish, exit code, output tail or log reference, commit or
workspace identity, and status. It produces Evidence and normally a generic
validation-report Artifact.

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
