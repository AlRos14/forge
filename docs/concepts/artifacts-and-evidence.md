# Artifacts and evidence

## Artifact

Artifact is the generic durable output primitive:

~~~text
Artifact
  id
  task_id
  storage_kind: inline | external
  content xor content_ref
  kind
  content or external/path reference
  metadata
  digest?
  created_at

artifact_execution_producer
  artifact_id -> Artifact
  execution_id -> Execution
~~~

PR4 supports only an Execution producer. Its Actor is derived through
`ArtifactExecutionProducer -> Execution -> ActorRef`, not duplicated on the
Artifact. Deterministic ValidationRun production is deferred to PR8, which may
add an `artifact_validation_run_producer` relation without rebuilding the
Artifact table. PR4 does not store a future ValidationRun FK or use a synthetic
System Actor.

Initial kinds include plan, review report, validation report, diff, patch,
summary, design document, investigation, API contract, and test report. The
kind is descriptive; it does not create a separate cognitive subsystem.

Inline content and external `content_ref` are mutually exclusive. Content may
remain in an appropriate file/object store while SQLite stores metadata,
authorization, reference, and digest. `content_ref` is an internal locator,
not a bearer capability; PR4 responses omit it, and every read is authorized
through Task to Project before Artifact data is returned. Artifact rows,
producer relations, and producer identity are immutable.

## PR4 authority boundary

Artifacts created through the PR4 generic API are generic authority. Legacy
planning, review, validation, Project documents, and other verticals remain
their own authority until their assigned migration. No legacy writer implicitly
creates an Artifact, and PR4 does not silently project legacy rows.

## Evidence

Evidence is a typed observation used by a Gate, projection, or audit. It
records what was observed, by which Actor Execution, ValidationRun, or other
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
