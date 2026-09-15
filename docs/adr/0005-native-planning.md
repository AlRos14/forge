# ADR 0005: Harness-native planning

Status: Accepted in PR 0

## Context

A Forge-owned planner duplicates harness cognition and makes a generic
read-only permission mode look like native planning.

## Decision

Planning is an Execution with purpose plan. The adapter invokes native harness
planning when available; a Human uses the same planning role through the
control plane. The result is a generic plan Artifact.

## Consequences

Capability reporting must distinguish native, emulated, and unsupported.
Forge stores what was produced without rewriting it into a canonical plan
engine.

## Migration

PRs 2, 3, and 7 separate purpose from permission, expose capability truth, and
remove canonical planner retry/checklist machinery.
