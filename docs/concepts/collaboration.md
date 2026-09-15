# Collaboration primitives

Collaboration is durable, explicit, and intentionally small. The initial
primitives are Message, Handoff, Proposal, and Decision.

## Message

A Message communicates between Actor-to-Actor, Actor-to-Role, or a Task scope.
It may reference a WorkUnit, Execution, Artifact, or Gate. Delivery status is
separate from the message body, and the body is data rather than authority.

## Handoff

A Handoff transfers or requests work or responsibility. Initial intents are
rework, delegation, question, answer, investigate, and decision_request.
Handoffs identify source and target Actor or Role, relevant Task/WorkUnit,
parent Execution, Artifact references, expected policy, and status.

Reviewer feedback uses a ReviewReport Artifact plus a Handoff or Message. If
the same implementer continues, the Handoff targets that Actor and explicit
HarnessSession where possible.

## Proposal

A Proposal records a consequential action before it happens. It includes the
proposer, target, action, reason, relevant versions/digests, required policy,
and status. Examples include stopping an Execution, reassigning a WorkUnit,
discarding a workspace, or merging changes.

## Decision

A Decision resolves a Proposal. It records the deciding Actor or Actors,
policy, outcome, rationale, timestamp, and the Proposal version resolved.
Approve, reject, and counter/supersede are sufficient initial outcomes.

The system does not infer approval from silence, chat tone, model confidence,
or an unrelated Task transition.

## Coordination and events

The TaskRole coordination mode and policy determine whether one member,
leader, majority, unanimous members, or a Human is required. There is no
global two-person consensus rule and no expression-language DSL.

Creation and resolution publish durable, redacted domain events. Event payloads
contain authorized IDs, versions, and bounded status, never credentials,
hidden prompts, filesystem paths, workspace handles, or raw model context.

Messages may be grouped by a lightweight transport Thread if needed. A Thread
is context grouping only; it is not another authority hierarchy or Agent class.
