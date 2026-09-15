# Actors

An Actor is the universal participant in task work. The initial Actor kinds are
Human and Agent. Both are persisted references that can be members of any
TaskRole and can produce Executions, Messages, Handoffs, Proposals, Decisions,
Artifacts, and Evidence.

## Actor reference

The initial cross-domain reference is intentionally small:

~~~text
ActorRef
  kind: Human | Agent
  id: opaque identifier
~~~

It is a reference, not a capability. Every service boundary authorizes the
referenced Actor against the account, Project, Task, RoleMembership, and
current policy before disclosing or mutating anything.

## Human

A Human is a first-class Actor. Human work does not use a fake Agent,
HarnessProfile, or HarnessSession. A Human planner, implementer, reviewer, and
orchestrator is represented by an ordinary Execution under the corresponding
Role and Purpose.

Human-specific UI ergonomics are allowed. Human-specific domain shortcuts that
change authority or bypass the Execution, Artifact, Gate, workspace, or
collaboration model are not.

## Agent

An Agent is a persistent AI Actor bound to one HarnessProfile. Harness identity
is part of the Agent's identity. A model label alone is insufficient to
identify an Agent.

Agent configuration changes are versioned or create a new Agent. Existing
Executions keep their configuration and capability snapshots. A later profile
change never rewrites historical work.

## Actor lifecycle

Membership, availability, suspension, deletion, and replacement are explicit
domain operations. Removing an Agent must preserve historical Executions and
must either invalidate future memberships safely or record a deliberate
replacement. It must not erase evidence or make historical Actor references
ambiguous.

See [agents-and-harnesses.md](agents-and-harnesses.md) for the Agent-specific
identity boundary and [roles.md](roles.md) for membership.
