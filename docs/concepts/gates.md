# Gates

A Gate is a deterministic constraint on a Task, WorkUnit, merge, or lifecycle
operation. It does not contain model cognition. PR9 stores a scoped Gate
identity, immutable versioned policy revisions, and immutable evaluations.
Unknown policy schemas or invalid policy digests fail closed.

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

Each required input names an exact record and carries its version, digest,
status, producer, subject, and provenance. Evaluation never resolves `latest`.
A stale or missing input blocks the Gate rather than being silently
substituted. The same policy revision and normalized exact input set deduplicate
to one durable evaluation; a new input set creates a separate historical
evaluation.

## Review and validation

Validation and review are independent Gate inputs. A passing CI command does
not pass a required reviewer Gate. A passing reviewer does not claim a CI
command ran.

The bounded PR9 review policy supports one acceptable reviewer, all required
reviewers, or an N-of-M quorum, with ActorRef, Human/Agent, and frozen TaskRole
membership constraints. A required failed report is not hidden by another
passing report. Its exact ReviewReport remains a cognitive fact and makes the
Gate unsatisfied. The separate retry-budget consumer that should turn that
failure into a rework handoff is pending in this PR9 branch; Gate evaluation
itself does not consume retries or create an Execution.

## Authority

Gates enforce deterministic authority around leases, workspace writes,
credentials, merge, cancellation, reassignments, and Human decisions. Models
cannot bypass them by proposing a different state transition.

The Gate record identifies its Task and scope. A GateEvaluation records the
policy revision, exact input set, deterministic result, and timestamp. Review
reports, ValidationRuns/Evidence, Decisions, and WorkUnit facts remain owned by
their source records. Merge admission requires a current satisfied
merge-readiness evaluation and the existing serialized TaskIntegrationOperation
authority. Retry budgets remain lifecycle/orchestration policy and are never
Gate inputs. REST/MCP/UI projection replacement is deferred to PR12.

For merge readiness, exact ReviewReport and ValidationRun inputs must agree on
the same Workspace, commit, and snapshot. A Task with WorkUnits must include an
exact successful WorkUnitIntegration input. A direct Task merge also requires
the completed source Execution in that Workspace. Before merge admission,
MergeService holds the shared workspace lock and checks that the merge source
still has the evaluated commit; a stale source leaves the Task in
`ready_to_merge` without creating a merge operation. The existing clean-tree
check still runs before Git integration.
