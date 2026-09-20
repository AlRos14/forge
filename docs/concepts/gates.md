# Gates

A Gate is a deterministic constraint on a Task, WorkUnit, merge, or lifecycle
operation. It does not contain model cognition.

## Gate inputs

A Gate may require:

* validation Evidence from a ValidationRun or an Actor-driven validation
  Execution, or a named command that Forge materializes as a ValidationRun;
* one or more reviewer Artifacts and verdicts;
* a Human action;
* a Proposal Decision;
* dependency completion;
* an active workspace or merge lease;
* security, credential, or repository policy;
* a bounded custom deterministic check.

Each required input has an exact version, digest, status, and provenance. A
stale or missing input blocks the Gate rather than being silently substituted.

## Review and validation

Validation and review are independent Gate inputs. A passing CI command does
not pass a required reviewer Gate. A passing reviewer does not claim a CI
command ran.

Gates can require several reviewers under independent, partitioned, or
collaborative coordination. A review failure creates a rework collaboration
path; it does not encode a hidden prompt or mutate the Task into a cognitive
state.

## Authority

Gates enforce deterministic authority around leases, workspace writes,
credentials, merge, cancellation, reassignments, and Human decisions. Models
cannot bypass them by proposing a different state transition.

The Gate record identifies the policy, required inputs, evaluated result,
evaluating process, and timestamp. Readiness is computed from current
authoritative records; it is not approval by itself.
