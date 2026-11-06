# ADR 0005: Harness-native planning

Status: Accepted in Plan PR0; purpose wording superseded by ADR 0014.

## Context

A Forge-owned planner duplicates harness cognition and makes a generic
read-only permission mode look like native planning.

## Decision

Planning is an ordinary Execution under its explicit Role and WorkUnit scope.
The adapter invokes native harness planning when available; a Human uses the
same planning responsibility through the control plane. The result is a
generic plan Artifact. No execution-purpose classification is required.

## Consequences

Capability reporting must distinguish native, emulated, and unsupported.
Forge stores what was produced without rewriting it into a canonical plan
engine.

## Migration

Plan PRs 2, 3, and 7 separate purpose from permission, expose capability
truth, and remove canonical planner retry/checklist machinery.
