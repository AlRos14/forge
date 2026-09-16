# Harness sessions

HarnessSession is the durable identity of an external harness conversation or
execution continuity context:

~~~text
HarnessSession
  id
  agent_id
  harness_kind
  external_session_id
  profile_snapshot
  capabilities_snapshot
  workspace_scope?
  status
  created_at
  last_used_at
  closed_at?
~~~

The owning Agent and harness are immutable. Profile and capability snapshots
describe what the session was created with; later Agent configuration changes
do not rewrite them.

## Explicit attachment

An Execution may reference a HarnessSession. Creating, attaching, resuming,
closing, and marking a session unavailable are explicit repository operations.
Parent Execution lineage is separate from session identity.

Rework targets the Actor and exact session selected by durable Handoff or
policy. A review session does not become the implementation session merely
because it reviews the same Task.

## Human work

Human Executions have no HarnessSession. The nullable reference is meaningful:
absence means human or non-session work, not an unknown synthetic session.

## Recovery

Process restart loads session metadata from durable storage and validates Agent,
harness, profile, workspace scope, and availability before resume. If the
external session cannot resume, the adapter reports unsupported or unavailable
and policy chooses queue, stop/resume, or a new explicit Execution.

No role-name lookup or latest-thread heuristic is a fundamental identity
mechanism. Any temporary compatibility reader is bounded to the owning Plan PR
that removes it.
