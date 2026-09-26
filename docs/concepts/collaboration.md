# Collaboration primitives

Collaboration is durable, explicit, and intentionally small. PR4 introduces
generic Message, Handoff, Proposal, and Decision records alongside Artifact.
They are authoritative for records created through their generic surface.
Legacy chats, handoffs, Project Decisions, and memory proposal/decision records
remain separate authorities until their assigned migration; there is no
implicit dual-write or silent projection.

## Message

A Message communicates between Actor-to-Actor, Actor-to-Role, or a Task scope.
PR4 associates it with Artifacts only; WorkUnit and Gate relations are deferred
to their later Plan PRs. Message is immutable. Delivery/read status is separate
from the body and is not introduced in PR4; the body is data rather than
authority.

## Handoff

A Handoff transfers or requests work or responsibility. Initial intents are
rework, delegation, question, answer, investigate, and decision_request.
Handoffs identify the creator, optional source Role, target Actor/Role/Task,
parent Execution, Artifact references, expected policy, and lifecycle status.
Addressing a Role or Agent does not create or change RoleMembership. PR4 does
not associate Handoffs with WorkUnits.

Reviewer feedback uses a ReviewReport Artifact plus a Handoff or Message. If
the same implementer continues, the Handoff targets that Actor and explicit
HarnessSession where possible.

## Proposal

A Proposal records an intent for a consequential action. PR4 supports Task,
Execution, and Workspace targets only. Content starts at version 1 and is
immutable; a material change is a new Proposal linked through
`supersedes_proposal_id`. Policy references/snapshots are evidence, not an
executable DSL, effective permission, or authorization. Proposal does not
dispatch or execute its action.

## Decision

A Decision resolves a Proposal. It records one or more Human and/or Agent
deciders, policy evidence, outcome, rationale, timestamp, and the Proposal
version resolved. PR4 outcomes are `approve`, `reject`, and `supersede`; a
counterproposal is a `supersede` Decision followed by a new Proposal. A
Decision records a choice and never executes the Proposal.

The system does not infer approval from silence, chat tone, model confidence,
or an unrelated Task transition.

## Coordination and events

The TaskRole coordination mode and policy determine whether one member,
leader, majority, unanimous members, or a Human is required. There is no
global two-person consensus rule and no expression-language DSL.

Creation and lifecycle changes append redacted records to the shared durable
`domain_event` ledger in the same transaction as the mutation. EventBus is
post-commit notification only. Payloads contain bounded IDs, kinds, actions,
outcomes, digests, and statuses, never content, message bodies, reasons,
rationales, storage locators, credentials, filesystem paths, workspace
handles, hidden prompts, or raw model context.

Messages may be grouped by a lightweight transport Thread if needed. A Thread
is context grouping only; it is not another authority hierarchy or Agent class.
