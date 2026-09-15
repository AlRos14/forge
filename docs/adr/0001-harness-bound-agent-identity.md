# ADR 0001: Harness-bound Agent identity

Status: Accepted in PR 0

## Context

A model label does not identify the tools, context loop, approvals, planning
mode, or session semantics used to perform work.

## Decision

An Agent is a persistent AI Actor bound to a stable harness identity and, when
credentials are identity-bearing, an explicit credential/account context. A
`HarnessProfileRevision` is a versioned configuration snapshot for future
runs: model, reasoning, approval, sandbox, and non-identity harness options.

Changing the harness identity, credential/account identity, or another
property that changes the persistent execution Actor creates another Agent.
Changing compatible run configuration creates a new profile revision while
preserving the Agent. Each Execution stores the exact effective profile,
credential context, and capability snapshot used.

## Consequences

Codex Sol and Cursor Sol are distinct Agents. Codex on account A and Codex on
account B are also distinct when the account context is identity-bearing.
Failover is visible as a new Actor/harness/account fact, and historical
Executions remain auditable.

## Migration

PRs 1–3 add ActorRef, profile identity rules, HarnessSession, and the adapter
capability boundary before old executor identity fields are removed in PR 13.
