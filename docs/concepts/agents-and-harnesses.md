# Agents and harnesses

## Harness-bound identity

An Agent is an AI Actor plus a stable harness identity and an effective
HarnessProfileRevision. The harness materially
changes:

* available tools and execution protocol;
* interaction loop and context handling;
* editing and approval mechanics;
* planning and review modes;
* session continuity and recovery;
* model steering and reasoning controls.

Therefore GPT-5.6 Sol in Codex and GPT-5.6 Sol in Cursor are distinct Agent
records. Reusing the same display name or model identifier must not collapse
them.

## Agent identity and profile revisions

An Agent identity includes the stable harness identity and, when credentials
are identity-bearing, an explicit credential/account context or reference. A
profile revision tunes future runs without silently rewriting that identity.
The conceptual shape is:

~~~text
Agent
  harness identity
  credential/account context?
  active HarnessProfileRevision
~~~

`HarnessProfileRevision` is a versioned execution configuration containing at
least:

~~~text
HarnessProfileRevision
  harness_kind
  model or model profile
  provider configuration reference
  approval and sandbox settings
  harness-specific configuration
  profile revision and digest
~~~

Secrets are referenced through the existing secure credential boundary; they
are not copied into public profiles or Execution context. An Execution
snapshots the effective profile and relevant configuration at start.

Changing the harness of an existing Agent creates another Agent. A compatible
profile revision can evolve future Executions without rewriting history.
Changing the Agent's credential/account identity creates another Agent when
that context materially changes which native account is used. Changing model,
reasoning effort, approval policy, sandbox settings, or other non-identity
harness arguments may create a new profile revision on the same Agent.

## HarnessSession (Plan PR2)

PR2 adds a generic `HarnessSession` as the durable continuity record for an
Agent in one opaque harness kind. It stores the optional external
harness-native session id, profile and capability snapshots, optional
workspace scope, predecessor, timestamps, and the minimal `pending`, `active`,
`ended`, and `failed` lifecycle. Agent and harness identity are immutable;
profile/capability snapshots are historical and are not rewritten when the
Agent's current profile changes.

Execution resume uses the explicit `Execution.harness_session_id` reference,
then the session's `external_session_id`. A session is not inferred from a
Role, Task, model, or latest Execution, and a session scoped to one workspace
is not silently reused in another. Human Executions have no HarnessSession.

The existing `agent_session` table and `AgentSession` model are deliberately
different. They preserve history from the retired embedded Agent Runtime and
remain readable for audit and connection-health projections. No new embedded
execution may create or resume one, and it is never the generic Execution
continuity authority. `agent_context_scope` remains the foreign-key scope for
server-owned Chat context manifests; those deny-workspace rows do not authorize
runtime tools. Protected runtime state, LCM state, and runtime context manifests
have no post-PR10 cognition consumer and remain historical until Plan PR13.

The final persistence shape is owned by Plan PR1/Plan PR2. Regardless of
representation, each Execution snapshots the exact effective profile,
credential context, and typed `harness_capabilities` used by the selected
adapter; later profile revisions never rewrite historical Executions. The
separate legacy Agent `capabilities_json` field remains authored tags and
filters. New HarnessSession capability snapshots preserve effective typed
support evidence historically.

## HarnessAdapter

The target adapter boundary is narrow:

~~~text
detect
capabilities
start
resume
cancel
events
usage
~~~

When the harness supports them, the adapter may also expose fork, steer, and
pause/resume. The core expresses platform intent; the adapter translates it
into native harness behavior.

## Capabilities

Capabilities are dimensional. Each dimension is explicitly classified as
`native`, `emulated`, `unsupported`, or `unknown`. Only native and emulated
support are available, and unknown fails closed. For example, a read-only
sandbox is not native planning unless the integration invokes a harness-native
planning mode. `PermissionPolicy::Plan` remains permission vocabulary and does
not itself prove native planning support.

Execution continuity is a generic `Start` or `Resume { external_session_id }`
intent derived from `Execution.harness_session_id` and the historical
HarnessSession. The adapter translates Resume into that harness's protocol.
An explicit Resume never falls through to another route candidate or silently
starts a fresh run. Start may use ordered fallback routing; the actual winner's
capabilities and candidate identity are recorded in the Execution snapshot.

The core must not implement a provider-specific reasoning loop, prompt
protocol, context manager, or pretend-native fallback. It may enforce
deterministic authority around an adapter invocation.

## Credentials and usage

Credential storage, environment injection, daemon lifecycle, usage snapshots,
quota, cooldown, and failover remain platform infrastructure. Failover may
select another Agent only when that fact is explicit in Execution history; it
must not impersonate the original Agent or mutate its identity.

Cursor's interactive `/usage` probe is not started as a periodic child of an
Execution. Its PTY command can run in a separate process session, outside the
Execution's verifiable process group. A Cursor Execution without a current
observation therefore has no `account_usage`; Forge does not substitute an
older account snapshot. Cursor's side-effect-free execution detection checks
the configured executable, while the detailed availability check remains a
separate operation.

## Frozen Execution host (Plan PR10)

Admission may resolve an Agent's pinned daemon or select an available daemon
for an unpinned Agent. The Execution snapshot stores that exact host as
`resolved_daemon_id`. Start, Resume, Cancel, graceful shutdown, and recovery
for that Execution use the stored ID. Changes to the Agent's current daemon
binding or daemon availability cannot silently move the Execution to another host. A missing or
invalid host fails closed. Reconnecting the same daemon is allowed and remains
subject to PR3's connection-generation checks.

## Agent Chat transition (Plan PR10)

Agent Chat messages, durable turn jobs, binding/profile provenance,
operating-context provenance, retry state, and historical reads remain
available as historical reads. PR11 now prevents all Main and Project Agent
Chat turn claims and invocations. Prompt text and a temporary directory do not
establish a no-filesystem boundary. The removed native typed Forge tool
catalog is not available through Chat. Their transcripts and binding history
are read-only pending PR12 surface removal and PR13 schema cleanup.
