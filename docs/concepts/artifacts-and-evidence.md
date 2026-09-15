# Artifacts and evidence

## Artifact

Artifact is the generic durable output primitive:

~~~text
Artifact
  id
  task_id
  producing_actor
  producing_execution?
  kind
  content or external/path reference
  metadata
  digest?
  created_at
~~~

Initial kinds include plan, review report, validation report, diff, patch,
summary, design document, investigation, API contract, and test report. The
kind is descriptive; it does not create a separate cognitive subsystem.

Content may remain in an appropriate file/object store while SQLite stores
metadata, authorization, reference, and digest. A path or external reference
is never a bearer capability and must be checked against the owning scope.

## Evidence

Evidence is a typed observation used by a Gate, projection, or audit. It
records what was observed, by which Actor, ValidationRun, or other
deterministic process, from which Execution/Workspace/commit when applicable,
when, and with what digest or log reference.

Validation results, review findings, integration conflicts, capability
observations, and usage snapshots can all be Evidence without requiring
bespoke persistence for each output category. A deterministic ValidationRun
is the producer for automated checks; it is not an Actor and does not need a
HarnessSession.

Evidence is not an approval. A model statement, Artifact, or green UI
projection cannot satisfy a Human Gate unless the Gate records the required
principal-bound action.

## Reproducibility

Artifact and Evidence producers, role, purpose, harness/profile/capability
snapshots, relevant workspace or commit, timestamps, and content digests are
preserved sufficiently for audit. Later edits append a new Artifact or
Evidence record; they do not rewrite historical outputs.

## Scope and retention

Every Artifact and Evidence reference is authorized to its Project and Task
before content, path, filename, checksum, or cursor data is disclosed.
Deletion, redaction, and release retention are explicit policies. Generic
Artifacts do not become a hidden memory or authority source.
