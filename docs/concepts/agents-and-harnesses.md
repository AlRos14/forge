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
different. They belong to the embedded Agent Runtime/Agent Host, together
with `agent_context_scope`, protected runtime state, and context manifests.
They remain legacy embedded-runtime infrastructure until the named later
cleanup; they are not the generic Execution continuity authority.

The final persistence shape is owned by Plan PR1/Plan PR2. Regardless of
representation, each Execution snapshots the exact effective profile,
credential context, and capabilities used; later profile revisions never
rewrite historical Executions.

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

Capabilities are dimensional. Relevant support is explicitly classified as
native, emulated, or unsupported. For example, a read-only sandbox is not
native planning unless the harness exposes a planning mode through the
integration. An unknown Cursor or Codex feature remains unknown until detected.

The core must not implement a provider-specific reasoning loop, prompt
protocol, context manager, or pretend-native fallback. It may enforce
deterministic authority around an adapter invocation.

## Credentials and usage

Credential storage, environment injection, daemon lifecycle, usage snapshots,
quota, cooldown, and failover remain platform infrastructure. Failover may
select another Agent only when that fact is explicit in Execution history; it
must not impersonate the original Agent or mutate its identity.
