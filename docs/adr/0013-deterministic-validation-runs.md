# ADR 0013: Deterministic validation is a ValidationRun

Status: Accepted in Plan PR0

## Context

An Execution has exactly one Actor, Role, and Purpose, while automated tests,
typechecks, builds, lint, and scanners are core-controlled evidence rather
than model cognition. Inventing a System Actor would make deterministic
authority look like human or Agent work and would blur validation with review.

## Decision

Automated or trusted deterministic checks are recorded as `ValidationRun`.
They record the check/command, bounded environment and configuration summary,
workspace/commit identity, timestamps, status, exit code, and log reference.
A ValidationRun produces Evidence and may produce a generic validation-report
Artifact through `ArtifactProducer::ValidationRun`, without requiring an
Actor, HarnessSession, or fake System Actor. Actor-produced Artifacts instead
use `ArtifactProducer::Execution` and derive their Actor from that Execution.

Human or Agent investigation, reproduction, or interpretation remains an
ordinary Execution with purpose `validate` or `investigate`. Review remains a
separate reviewer Execution. Validation and review remain independent Gate
inputs.

## Migration

Plan PR8 introduces the durable validation/review representation and removes
the special review runtime after all readers and writers move. Plan PR0 only
freezes the distinction; it adds no schema or runtime behavior.
