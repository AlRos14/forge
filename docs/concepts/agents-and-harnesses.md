# Agents and harnesses

## Harness-bound identity

An Agent is an AI Actor plus a specific HarnessProfile. The harness materially
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

## HarnessProfile

A HarnessProfile is a versioned execution configuration containing at least:

~~~text
HarnessProfile
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
