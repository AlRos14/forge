# ADR 0008: Small durable collaboration vocabulary

Status: Accepted in PR 0

## Context

Cross-harness collaboration needs durable semantics, but a general
coordination operating system would recreate the complexity being removed.

## Decision

Use Message, Handoff, Proposal, and Decision. Add a lightweight Thread only
if transport grouping requires it; it is never an authority hierarchy.

## Consequences

Review rework, questions, delegation, and orchestrator disagreement become
visible domain records without hidden prompt rewriting.

## Migration

PR 4 adds the primitives and redacted events. PRs 6 and 8 use them for
orchestration and review feedback.
