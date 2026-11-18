# Orchestrated delivery

Forge 0.9.1 coordinates Human and harness-bound Agent work through durable
Tasks, WorkUnits, Executions, evidence, and lifecycle transitions. Autonomy
comes from explicitly authorized routes and deterministic services; a harness
does not grant itself new roles, repository scope, or external action rights.

~~~~mermaid
flowchart LR
  H[Human defines scope and mode] --> C[Coordinator prepares and routes work]
  C --> W[Assigned WorkUnits are dispatched]
  W --> R[Review, validation, and Gate]
  R -->|accepted exact evidence| D[Integration and PR delivery]
  R -->|finding or failed check| C
  C -->|scope or authority change| HD[Human decision]
  HD --> C
  D -->|trusted PR feedback| C
  D -->|merge| HM[Human operation]
~~~~

## From objective to delivery

A Project provides role defaults and eligible Actors. A Task captures its
current team and authorization. In proposal mode, a workspace-free coordinator
turn returns an inspectable plan; a Human approves the exact plan before
materialization. Prepare-and-execute can materialize a plan inside its envelope
and record dispatch intent. Manual mode keeps explicit WorkUnit preparation.

Materialization validates the exact Task version, authorization, team, envelope,
plan digest, role and Actor membership, dependencies, review subjects, and
Artifact inputs. It creates WorkUnits with a durable receipt. The dispatcher
admits only assigned, eligible Agent WorkUnits. Human WorkUnits remain pending
Human work. Repository WorkUnits use isolated workspaces and leases; integration
is serialized separately.

ReviewReports express Human or Agent reviewer judgment over an exact result.
ValidationRuns record deterministic checks and outputs. GateEvaluations apply
versioned policy to exact evidence. Gate policy schema 2 can require a passed
ValidationRun pinned to the exact catalog check key and definition digest.
Review, validation, Gate evaluation, integration, and merge remain separate
facts. A coordinator cannot rewrite a review verdict or satisfy a Gate by
describing it in text.

## Bounded coordination and intervention

Coordinators can triage exact review findings, validation findings, and eligible
PR feedback through typed requests. A failed check can route to correction,
recheck, or a Human decision. A request for validation can select only a check
pinned in the current Task envelope; Forge executes the command from the
immutable catalog definition. Human decisions bind exact source evidence and
expected versions. Scope or authority changes require Project owner/admin
authority before materialization.

Coordination envelope v2 can preauthorize up to three fallback Agents per
TaskRole, a micro-fix route and formal reviewer, and coordinator replanning.
Fallback is applied by the dispatcher only after an eligible unavailable route
is exhausted; uncertain Provider Starts never trigger reassignment. A micro-fix
is tied to exact findings and paths and is bounded to three files, sixty changed
lines, and at most three micro-fixes per Task. It always returns through
independent formal review and a fresh Gate. A replan is classified against the
authorized objective, team, WorkUnits, budget, repository/path scope, and
envelope; at most two are recorded per Task, and scope or authority changes
wait for a Human decision.

The [PR feedback loop](pr-feedback-loop.md) uses a Repo owner/admin policy and a
GitHub SCM credential handle to admit and triage external feedback. Same-PR
rework follows the regular WorkUnit and Gate path. External push or summary
actions are Human-approved by default and automatic only when the frozen
repository policy explicitly allows them. Forge has no merge action.

## Authority and evidence boundaries

TaskLifecycle remains the sole Task progress authority. Project Copilot is a
separate Project-scoped assistant; it does not receive Task worker or
coordination authority. A Proposal, Decision, validation result, or event
projection cannot independently grant a Worker assignment, WorkspaceLease,
Gate pass, or merge.

Every change of stage can be inspected through durable records and exact source
references. Public SSE events contain bounded identifiers, digests, states, and
allowlisted facts; they do not carry credentials, provider feedback bodies,
prompts, or unrestricted plan content.

## Verification boundary

The coordinator verification plan records full CI PASS for phase 1 and phase 2.
Its phase-3 implementation notes mark new builds and tests NOT RUN. Acceptance
with a real GitHub provider is also NOT RUN. See the [coordinator verification
and PR loop plan](../plans/coordinator-verification-and-pr-loop.md) and
[autonomous delivery fluency plan](../plans/autonomous-delivery-fluency.md) for
the phase evidence.
