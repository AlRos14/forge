# ADR 0001: Harness-bound Agent identity

Status: Accepted in PR 0

## Context

A model label does not identify the tools, context loop, approvals, planning
mode, or session semantics used to perform work.

## Decision

An Agent is a persistent AI Actor bound to a HarnessProfile. A harness change
creates another Agent. Each Execution stores the exact profile and capability
snapshot used.

## Consequences

Codex Sol and Cursor Sol are distinct Agents. Failover is visible as a new
Actor/harness fact, and historical Executions remain auditable.

## Migration

PRs 1–3 add ActorRef, profile identity rules, HarnessSession, and the adapter
capability boundary before old executor identity fields are removed in PR 13.
