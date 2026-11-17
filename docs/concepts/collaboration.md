# Collaboration primitives

Collaboration is durable, explicit, and intentionally small. PR4 introduces
generic Message, Handoff, Proposal, and Decision records alongside Artifact.
They are authoritative for records created through their generic surface.
Legacy chats, handoffs, Project Decisions, and memory proposal/decision records
remain separate authorities until their assigned migration; there is no
implicit dual-write or silent projection.

Opaque IDs are references, never authority. A caller-supplied reference is
scoped to its owning Task before foreign semantic content, state, or corruption
is loaded. Missing and cross-Task references are indistinguishable within each
reference contract. Resource references generally resolve as not found, while
membership or derived-identity claims may use their uniform authorization or
contract error. A corrupt same-Task record continues to fail closed.

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
the same implementer continues, a PR4 Handoff can target that Actor, Role, or
Task. PR4 does not persist the recipient's exact HarnessSession. If PR6 or PR8
needs session continuity for rework, it must add an explicit additive relation
or typed action carrying that identity; it must never infer continuity from a
Role or latest-Execution lookup.

## Proposal

A Proposal records an intent for a consequential action. PR4 supports Task,
Execution, and Workspace targets only. Content starts at version 1 and is
immutable; a material change is a new Proposal linked through
`supersedes_proposal_id`. Policy references/snapshots are evidence, not an
executable DSL, effective permission, or authorization. Proposal does not
dispatch or execute its action.

## Decision

A Decision resolves a Proposal. It records the deciders, outcome, rationale, timestamp, and exact Proposal version. A generic Decision records a choice; it does not itself execute the Proposal or grant blanket permission. A policy-aware domain service may consume an exact Decision only when that service separately verifies the operation's authority and current policy. The PR external-action service is one such explicit consumer for actions tied to a frozen repository policy.

The system does not infer approval from silence, chat tone, model confidence,
or an unrelated Task transition.

## Coordination and events

TaskRole coordination modes and policy describe the coordination contract. The generic collaboration surface does not enforce member, leader, majority, unanimity, or Human requirements when recording a Decision. A domain service must evaluate the relevant policy before any protected effect. PR external actions use their dedicated service, versioned action record, frozen repository policy, and Human decision routes; generic Proposal and Decision records do not create a second path to those effects. There is no global two-person consensus rule or expression-language DSL in PR4.

Creation and lifecycle changes append records to the shared durable
`domain_event` ledger in the same transaction as the mutation. EventBus is
post-commit notification only. Payloads are bounded to server-created IDs and
Task IDs, enum kinds, Handoff intent/status/version, Proposal target kind,
Decision outcome, and lifecycle values. They omit free-form Artifact digests,
Proposal actions, Artifact content and `content_ref`, Message bodies, Proposal
reasons and policy references, Decision rationales and policy references,
filesystem paths, credentials, workspace handles, hidden prompts, and raw
model context.

Messages may be grouped by a lightweight transport Thread if needed. A Thread
is context grouping only; it is not another authority hierarchy or Agent class.
