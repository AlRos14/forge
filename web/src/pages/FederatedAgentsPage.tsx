import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, useSearch } from '@tanstack/react-router'
import {
  ArrowUpRight,
  CaretRight,
  CheckCircle,
  CircleNotch,
  Copy,
  Key,
  MagnifyingGlass,
  Plus,
  Robot,
  ShieldCheck,
  TerminalWindow,
  WarningCircle,
} from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { useAgentChatsQuery } from '@/features/agent-chat/hooks'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import {
  useAgentProfilesQuery,
  useAgentProviderCapabilitiesQuery,
  useAgentSessionsQuery,
  useCancelProviderAuthorizationMutation,
  useConnectEmbeddedProfileMutation,
  useCreateEmbeddedAgentMutation,
  useCreateProviderEntryMutation,
  useFederatedAgentsQuery,
  useProviderAuthorizationQuery,
  useProvidersQuery,
  useRegisterHarnessAgentMutation,
  useRemoveProviderEntryMutation,
  useRenameProviderEntryMutation,
  useSelectAgentProfileMutation,
  useStartProviderAuthorizationMutation,
  isVersionConflict,
} from '@/features/federation/hooks'
import { testProviderEntry } from '@/features/federation/api'
import type { FederatedAgent } from '@/features/federation/types'
import type {
  AgentProviderCapability,
  CliRuntimeEntryResponse,
  ProviderCredentialMethod,
  ProviderEntryResponse,
  ProviderEntryTestResponse,
  ProviderRuntimeCapability,
} from '@/types/generated'
import {
  EmptyPanel,
  ErrorPanel,
  LoadingPanel,
  PageHeader,
  SectionKicker,
  StateBadge,
  StatusDot,
} from '@/features/federation/components'
import { ContextManifestDialog } from '@/features/federation/ContextManifestInspector'
import { MainAgentBindingCard } from '@/components/settings/MainAgentBindingCard'
import { ProjectAgentTab } from '@/components/settings/ProjectAgentTab'

const EMPTY_AGENTS: FederatedAgent[] = []
const EMPTY_CHAT_ENTRIES: AgentChatEntry[] = []
const EMPTY_ENTRIES: ProviderEntryResponse[] = []
const EMPTY_CLI_RUNTIMES: CliRuntimeEntryResponse[] = []

const DEFAULT_CEILING = {
  allowed: [
    'read_account',
    'read_project',
    'read_agent_chat',
    'read_task',
    'read_memory',
    'propose_task',
    'propose_message',
    'propose_review',
    'task_read',
    'task_write',
  ],
}

const runtimeDisplayNames: Record<string, string> = {
  direct: 'Direct · built-in runtime',
  codex: 'Codex CLI harness',
  claude_code: 'Claude Code harness',
  cursor: 'Cursor harness',
  gemini: 'Gemini CLI harness',
  opencode: 'OpenCode harness',
}

function humanize(value: string | null | undefined): string {
  if (!value) return 'Unknown'
  return value.replaceAll('_', ' ').replace(/\b\w/g, (letter) => letter.toUpperCase())
}

function shortId(value: string | null | undefined): string {
  if (!value) return 'Not recorded'
  return value.length > 18 ? `${value.slice(0, 9)}…${value.slice(-7)}` : value
}

function allowedPolicyValues(policy: Record<string, unknown> | null | undefined): string[] {
  const allowed = policy?.allowed
  return Array.isArray(allowed)
    ? allowed.filter((value): value is string => typeof value === 'string')
    : []
}

function capabilityEnabled(capabilities: Record<string, unknown>, key: string): boolean {
  return capabilities[key] === true
}

/** The capability-catalog method entry matching a stored provider entry. */
function catalogMethodForEntry(
  capabilities: AgentProviderCapability[] | undefined,
  entry: ProviderEntryResponse,
) {
  const capability = capabilities?.find((item) => item.provider === entry.provider)
  if (!capability) return undefined
  return entry.credential_method === 'api_key'
    ? capability.credential_methods.find((method) => method.method === 'api_key')
    : capability.credential_methods.find((method) => method.method !== 'api_key')
}

function runtimeOptionsForEntry(
  capabilities: AgentProviderCapability[] | undefined,
  entry: ProviderEntryResponse,
): ProviderRuntimeCapability[] {
  return (
    catalogMethodForEntry(capabilities, entry)?.runtimes ?? [
      { runtime: 'direct', support_level: 'stable', reason: null },
    ]
  )
}

function BoundAgentScopes({
  entries,
  isLoading,
  isError,
  onRetry,
}: {
  entries: AgentChatEntry[]
  isLoading: boolean
  isError: boolean
  onRetry: () => void
}) {
  if (isLoading) return <LoadingPanel label="Loading Main and Project Agent bindings" />
  if (isError) {
    return (
      <ErrorPanel
        title="Agent binding projection unavailable"
        description="Forge could not load the current Main and Project Agent scopes. Retry before relying on this view."
        onRetry={onRetry}
      />
    )
  }
  return (
    <section
      aria-labelledby="bound-agent-scopes-heading"
      className="overflow-hidden rounded-xl border border-border-subtle bg-card shadow-soft"
    >
      <header className="border-b border-border-subtle px-4 py-4 sm:px-5">
        <SectionKicker>Agent chat scopes</SectionKicker>
        <h2 id="bound-agent-scopes-heading" className="mt-1 text-lg font-semibold text-foreground">
          Main and Project Agent bindings
        </h2>
        <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
          These are the only durable chat owners. Task Workers and reviewers appear in Task detail,
          while unbound agents stay in the Agents tab.
        </p>
      </header>
      {entries.length > 0 ? (
        <div className="divide-y divide-ember-border">
          {entries.map((entry) => {
            const isMain = entry.kind === 'main'
            const label = isMain ? 'Global · Main' : (entry.project_name ?? 'Project Agent')
            const identity = entry.identity_name ?? 'Setup required'
            const status =
              entry.binding_state === 'active' ? entry.chat_status : entry.binding_state
            return isMain ? (
              <Link
                key={entry.chat_id}
                to="/chat"
                className="flex min-w-0 items-center justify-between gap-3 px-4 py-3 transition-colors hover:bg-background/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-foreground">{label}</p>
                  <p className="mt-1 truncate text-xs text-muted-foreground">
                    {identity} · account-owned timeline
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <StateBadge status={status} label={humanize(status)} />
                  <ArrowUpRight size={15} className="text-muted-foreground" aria-hidden />
                </div>
              </Link>
            ) : (
              <Link
                key={entry.chat_id}
                to="/projects/$projectId/chat"
                params={{ projectId: entry.project_id ?? '' }}
                className="flex min-w-0 items-center justify-between gap-3 px-4 py-3 transition-colors hover:bg-background/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-foreground">{label}</p>
                  <p className="mt-1 truncate text-xs text-muted-foreground">
                    {identity} · Project-owned timeline
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <StateBadge status={status} label={humanize(status)} />
                  <ArrowUpRight size={15} className="text-muted-foreground" aria-hidden />
                </div>
              </Link>
            )
          })}
        </div>
      ) : (
        <p className="px-4 py-4 text-sm text-muted-foreground">
          No Main or Project Agent binding is visible yet. Create an agent and choose its owning
          scope below.
        </p>
      )}
    </section>
  )
}

function AgentCard({
  agent,
  entries,
  selected,
  onSelect,
  onAddProfile,
}: {
  agent: FederatedAgent
  entries: ProviderEntryResponse[]
  selected: boolean
  onSelect: () => void
  onAddProfile: () => void
}) {
  const profilesQuery = useAgentProfilesQuery(selected ? agent.id : undefined)
  const sessionsQuery = useAgentSessionsQuery(selected ? agent.id : undefined)
  const selectProfile = useSelectAgentProfileMutation(agent.id)
  const [profileError, setProfileError] = useState<string | null>(null)
  const profiles = profilesQuery.data ?? []
  const sessions = sessionsQuery.data ?? []
  const activeSession = sessions.find(
    (session) => session.status === 'ready' || session.status === 'running',
  )
  const selectedProfile = profiles.find((profile) => profile.id === agent.profile_id)
  const selectedEntry = entries.find((entry) => entry.id === selectedProfile?.credential_handle_id)
  const requiresRecovery =
    selectedEntry != null && (selectedEntry.status === 'revoked' || selectedEntry.status === 'invalid')
  const connectionStatus = requiresRecovery
    ? 'recovery_required'
    : (activeSession?.connection_status ?? agent.effective_status)
  const runtime = agent.executor_type === 'embedded' ? 'direct' : agent.executor_type

  async function selectProfileVersion(profileId: string) {
    setProfileError(null)
    try {
      await selectProfile.mutateAsync({ profileId, version: agent.version })
    } catch (cause) {
      setProfileError(
        isVersionConflict(cause)
          ? 'Agent changed in another session. Refresh the roster before selecting a profile.'
          : cause instanceof Error
            ? cause.message
            : 'Profile selection failed.',
      )
    }
  }

  return (
    <Card
      className={`overflow-hidden border-border-subtle transition-shadow hover:shadow-card-hover ${selected ? 'border-ember-border shadow-ember' : ''}`}
    >
      <button
        type="button"
        className="flex w-full items-start gap-3 p-4 text-left"
        onClick={onSelect}
        aria-expanded={selected}
      >
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-ember-surface text-primary">
          <Robot size={18} weight="duotone" aria-hidden />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="truncate text-sm font-semibold text-foreground">{agent.name}</h2>
            <StateBadge
              status={agent.effective_status ?? agent.status}
              label={humanize(agent.effective_status ?? agent.status)}
            />
          </div>
          <p className="mt-1 truncate font-mono text-xs text-muted-foreground">
            {runtimeDisplayNames[runtime] ?? humanize(runtime)} ·{' '}
            {agent.model ?? 'profile pending'}
          </p>
          <p className="mt-3 text-xs text-muted-foreground">
            {agent.provider ? `${humanize(agent.provider)} provider` : 'CLI-managed login'} ·{' '}
            {agent.total_runs} runs
          </p>
        </div>
        <CaretRight
          size={16}
          className={`mt-1 shrink-0 text-muted-foreground transition-transform ${selected ? 'rotate-90 text-primary' : ''}`}
          aria-hidden
        />
      </button>

      {selected ? (
        <div className="border-t border-border-subtle bg-muted/20 px-4 py-4">
          <div className="grid gap-4 md:grid-cols-[1fr_1fr]">
            <div>
              <SectionKicker>Identity</SectionKicker>
              <dl className="mt-2 space-y-2 text-xs">
                <div className="flex justify-between gap-4">
                  <dt className="text-muted-foreground">Stable ID</dt>
                  <dd className="truncate font-mono text-foreground">{agent.id}</dd>
                </div>
                <div className="flex justify-between gap-4">
                  <dt className="text-muted-foreground">Current profile</dt>
                  <dd className="truncate font-mono text-foreground">{agent.profile_id}</dd>
                </div>
                {selectedEntry ? (
                  <div className="flex justify-between gap-4">
                    <dt className="text-muted-foreground">Provider entry</dt>
                    <dd className="truncate text-foreground">{selectedEntry.label}</dd>
                  </div>
                ) : null}
                <div className="flex justify-between gap-4">
                  <dt className="text-muted-foreground">Connection</dt>
                  <dd className="inline-flex items-center gap-1.5 text-foreground">
                    <StatusDot status={connectionStatus} />
                    {humanize(connectionStatus)}
                  </dd>
                </div>
              </dl>
              {requiresRecovery ? (
                <p
                  className="mt-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning"
                  role="status"
                >
                  This agent&apos;s provider entry is disconnected. Publish a profile on another
                  entry before relying on its Main or Project binding.
                </p>
              ) : null}
              <div className="mt-4 border-t border-border-subtle pt-3">
                <p className="text-xs text-muted-foreground">Profile tool ceiling</p>
                <div className="mt-2 flex flex-wrap gap-1.5" aria-label="Profile tool ceiling">
                  {allowedPolicyValues(selectedProfile?.tool_policy).length > 0 ? (
                    allowedPolicyValues(selectedProfile?.tool_policy).map((capability) => (
                      <span
                        key={capability}
                        className="rounded bg-muted px-2 py-1 font-mono text-micro text-foreground"
                      >
                        {capability}
                      </span>
                    ))
                  ) : (
                    <span className="text-xs text-muted-foreground">
                      Unavailable in this projection.
                    </span>
                  )}
                </div>
                <p className="mt-2 text-micro leading-5 text-muted-foreground">
                  This is a ceiling, not a grant. Effective permissions are recomputed for each
                  account, Main Agent Chat, Project Agent Chat, or Task scope.
                </p>
              </div>
            </div>
            <div>
              <SectionKicker>Profiles & sessions</SectionKicker>
              {profilesQuery.isLoading || sessionsQuery.isLoading ? (
                <p className="mt-2 text-xs text-muted-foreground">Loading continuity state…</p>
              ) : null}
              {profilesQuery.isError || sessionsQuery.isError ? (
                <p className="mt-2 text-xs text-destructive">Continuity state unavailable.</p>
              ) : null}
              {profileError ? (
                <p className="mt-2 text-xs text-destructive" role="alert">
                  {profileError}
                </p>
              ) : null}
              <div className="mt-2 space-y-2">
                {profiles.map((profile) => (
                  <div
                    key={profile.id}
                    className="flex items-center justify-between gap-3 rounded-md border border-border-subtle bg-card px-3 py-2"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-xs font-medium text-foreground">
                        {profile.provider ?? profile.executor_type} ·{' '}
                        {profile.model ?? 'unknown model'}
                      </p>
                      <p className="mt-0.5 font-mono text-micro text-muted-foreground">
                        v{profile.version} ·{' '}
                        {profile.id === agent.profile_id ? 'selected' : 'available'}
                      </p>
                    </div>
                    {profile.id !== agent.profile_id ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={selectProfile.isPending}
                        onClick={() => void selectProfileVersion(profile.id)}
                      >
                        Select
                      </Button>
                    ) : (
                      <span className="font-mono text-micro uppercase text-primary">Current</span>
                    )}
                  </div>
                ))}
                {sessions.length === 0 && !sessionsQuery.isLoading ? (
                  <p className="text-xs text-muted-foreground">No scoped sessions yet.</p>
                ) : null}
                {sessions.map((session) => (
                  <div key={session.id} className="rounded-md border border-border-subtle bg-card">
                    <div className="flex flex-wrap items-center justify-between gap-3 px-3 py-2">
                      <div className="min-w-0">
                        <p className="truncate text-xs font-medium text-foreground">
                          Context scope{' '}
                          <span className="font-mono">{shortId(session.context_scope_id)}</span>
                        </p>
                        <p className="mt-0.5 font-mono text-micro text-muted-foreground">
                          {session.backend_kind} · v{session.version}
                        </p>
                      </div>
                      <div className="flex items-center gap-2">
                        <StateBadge status={session.status} label={humanize(session.status)} />
                        <ContextManifestDialog
                          initialIdentityId={agent.id}
                          initialContextScopeId={session.context_scope_id}
                          label="Inspect context"
                          contextHint="this session"
                        />
                      </div>
                    </div>
                    <div
                      className="flex flex-wrap gap-1.5 border-t border-border-subtle px-3 py-2"
                      aria-label={`Capabilities for context scope ${shortId(session.context_scope_id)}`}
                    >
                      {[
                        ['persistent_session', 'Persistent session'],
                        ['protected_checkpoints', 'Protected checkpoints'],
                        ['lcm', 'Scoped continuity'],
                        ['steer', 'Steering'],
                        ['cancel', 'Cancellation'],
                      ].map(([key, label]) => {
                        const supported = capabilityEnabled(session.capabilities, key)
                        return (
                          <span
                            key={key}
                            className={`rounded border px-2 py-1 font-mono text-micro ${supported ? 'border-success/30 bg-success/10 text-success' : 'border-border-subtle bg-muted text-muted-foreground'}`}
                          >
                            {label}: {supported ? 'available' : 'unavailable'}
                          </span>
                        )
                      })}
                      <span className="rounded border border-border-subtle bg-muted px-2 py-1 font-mono text-micro text-muted-foreground">
                        Filesystem:{' '}
                        {session.capabilities.workspace === 'deny'
                          ? 'denied'
                          : String(session.capabilities.workspace ?? 'not reported')}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
          {agent.backend_kind === 'native' ? (
            <div className="mt-4 flex justify-end border-t border-border-subtle pt-3">
              <Button size="sm" variant="outline" onClick={onAddProfile}>
                Publish profile on another entry
              </Button>
            </div>
          ) : null}
          {agent.description ? (
            <p className="mt-4 border-t border-border-subtle pt-3 text-xs leading-5 text-muted-foreground">
              {agent.description}
            </p>
          ) : null}
        </div>
      ) : null}
    </Card>
  )
}

/** Live connectivity check for a stored provider entry. */
function ProviderConnectionTest({
  entryId,
  autoRun = false,
}: {
  entryId: string
  autoRun?: boolean
}) {
  const [pending, setPending] = useState(false)
  const [result, setResult] = useState<ProviderEntryTestResponse | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const runSeq = useRef(0)
  const autoRanFor = useRef<string | null>(null)

  const runTest = useCallback((id: string) => {
    const seq = (runSeq.current += 1)
    setPending(true)
    setFailure(null)
    testProviderEntry(id)
      .then((response) => {
        if (runSeq.current !== seq) return
        setResult(response)
      })
      .catch((cause: unknown) => {
        if (runSeq.current !== seq) return
        setResult(null)
        setFailure(cause instanceof Error ? cause.message : 'The connection test could not run.')
      })
      .finally(() => {
        if (runSeq.current === seq) setPending(false)
      })
  }, [])

  useEffect(() => {
    if (!autoRun || autoRanFor.current === entryId) return
    autoRanFor.current = entryId
    runTest(entryId)
  }, [autoRun, entryId, runTest])

  return (
    <div
      className="rounded-md border border-border-subtle bg-muted/20 px-3 py-2.5"
      role="status"
      aria-live="polite"
    >
      {pending ? (
        <span className="inline-flex items-center gap-2 text-xs text-muted-foreground">
          <CircleNotch size={14} className="animate-spin text-primary" aria-hidden />
          Testing the provider connection…
        </span>
      ) : result?.status === 'ok' ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="inline-flex items-center gap-1.5 text-xs font-medium text-success">
            <CheckCircle size={15} aria-hidden />
            Provider responding · {result.latency_ms} ms
            {result.message ? (
              <span className="font-normal text-muted-foreground">· {result.message}</span>
            ) : null}
          </span>
          <Button size="sm" variant="ghost" onClick={() => runTest(entryId)}>
            Test again
          </Button>
        </div>
      ) : result != null || failure != null ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="inline-flex items-center gap-1.5 text-xs font-medium text-destructive">
            <WarningCircle size={15} aria-hidden />
            {result?.message ?? failure ?? 'The connection test failed.'}
          </span>
          <Button size="sm" variant="outline" onClick={() => runTest(entryId)}>
            Retry test
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground">
            Check that this provider responds with the stored credential.
          </span>
          <Button size="sm" variant="outline" onClick={() => runTest(entryId)}>
            Test connection
          </Button>
        </div>
      )}
    </div>
  )
}

/**
 * OAuth operation runner for the wizard's Connect step; the server owns the
 * PKCE/device state and this panel renders only the public view.
 */
function ProviderAuthorizationPanel({
  capability,
  method,
  onConnected,
  onBack,
  onClose,
}: {
  capability: AgentProviderCapability
  method: ProviderCredentialMethod
  onConnected: (entryId: string | null) => void
  onBack: () => void
  onClose: () => void
}) {
  const [label, setLabel] = useState(`${capability.display_name} login`)
  const [operationId, setOperationId] = useState<string>()
  const [error, setError] = useState<string>()
  const start = useStartProviderAuthorizationMutation()
  const cancel = useCancelProviderAuthorizationMutation()
  const operation = useProviderAuthorizationQuery(operationId)
  const startInFlight = useRef(false)

  const operationState = operation.data?.state
  const operationEntryId = operation.data?.credential_handle_id
  useEffect(() => {
    if (operationState !== 'succeeded') return
    const timeoutId = window.setTimeout(() => onConnected(operationEntryId ?? null), 600)
    return () => window.clearTimeout(timeoutId)
  }, [onConnected, operationState, operationEntryId])

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (startInFlight.current) return
    startInFlight.current = true
    setError(undefined)
    try {
      const started = await start.mutateAsync({
        provider: capability.provider,
        method,
        redirect_origin: window.location.origin,
        credential_label: label.trim(),
        // The browser is on the server's machine whenever Forge is served over
        // loopback, so Forge itself binds the provider's localhost callback.
        // Anywhere else the server rejects browser OAuth and points at the
        // device-code method or `forge-ctl embedded provider login`.
        loopback_owner: 'server',
        loopback_port: null,
      })
      setOperationId(started.id)
      if (method === 'browser_oauth' && started.authorization_url) {
        window.location.assign(started.authorization_url)
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Provider authorization could not start.')
    } finally {
      startInFlight.current = false
    }
  }

  const current = operation.data
  const terminal = current
    ? ['succeeded', 'denied', 'expired', 'cancelled', 'failed'].includes(current.state)
    : false

  return (
    <>
      {!current ? (
        <form onSubmit={submit} className="mt-5 space-y-4">
          <div className="space-y-2">
            <Label htmlFor="oauth-label">Provider entry name</Label>
            <Input
              id="oauth-label"
              value={label}
              onChange={(event) => setLabel(event.target.value)}
              required
            />
          </div>
          {error ? (
            <p role="alert" className="text-xs text-destructive">
              {error}
            </p>
          ) : null}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onBack}>
              Back
            </Button>
            <Button type="submit" disabled={start.isPending}>
              {start.isPending ? 'Starting…' : 'Start authorization'}
            </Button>
          </DialogFooter>
        </form>
      ) : (
          <div className="mt-5 space-y-4" aria-live="polite">
            <div className="rounded-lg border border-border-subtle bg-muted/20 p-4">
              <div className="flex items-center justify-between gap-3">
                <SectionKicker>Authorization state</SectionKicker>
                <StateBadge status={current.state} label={humanize(current.state)} />
              </div>
              {current.user_code ? (
                <div className="mt-4">
                  <p className="text-xs text-muted-foreground">Enter this code at the provider:</p>
                  <button
                    type="button"
                    className="mt-2 flex w-full items-center justify-between rounded-md border border-input bg-card px-3 py-2 font-mono text-lg tracking-[0.16em] text-foreground"
                    onClick={() => void navigator.clipboard.writeText(current.user_code ?? '')}
                  >
                    {current.user_code}
                    <Copy size={16} aria-hidden />
                  </button>
                </div>
              ) : null}
              {current.authorization_url ? (
                <a
                  className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-primary hover:underline"
                  href={current.authorization_url}
                  target="_blank"
                  rel="noreferrer"
                >
                  Open provider authorization <ArrowUpRight size={14} aria-hidden />
                </a>
              ) : null}
              {current.error_message ? (
                <p className="mt-3 text-xs text-destructive" role="alert">
                  {current.error_message}
                </p>
              ) : null}
            </div>
            <DialogFooter>
              {!terminal ? (
                <Button
                  variant="outline"
                  disabled={cancel.isPending}
                  onClick={() =>
                    void cancel.mutateAsync({
                      id: current.id,
                      input: { expected_version: current.version },
                    })
                  }
                >
                  Cancel authorization
                </Button>
              ) : (
                <Button onClick={onClose}>Done</Button>
              )}
            </DialogFooter>
          </div>
        )}
    </>
  )
}

/** API-key entry form for the wizard's Connect step. */
function ApiKeyEntryForm({
  capability,
  onCreated,
  onBack,
}: {
  capability: AgentProviderCapability
  onCreated: (entry: ProviderEntryResponse) => void
  onBack: () => void
}) {
  const create = useCreateProviderEntryMutation()
  const [label, setLabel] = useState(`${capability.display_name} API key`)
  const [credential, setCredential] = useState('')
  const [baseUrl, setBaseUrl] = useState(capability.default_base_url ?? '')
  const [error, setError] = useState<string>()
  const inFlight = useRef(false)

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (inFlight.current) return
    if (!credential.trim() || !label.trim()) {
      setError('A name and API key are required.')
      return
    }
    inFlight.current = true
    setError(undefined)
    try {
      const entry = await create.mutateAsync({
        provider: capability.provider,
        label: label.trim(),
        credential: credential.trim(),
        base_url: baseUrl.trim() ? baseUrl.trim() : null,
      })
      setCredential('')
      onCreated(entry)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The provider entry could not be created.')
    } finally {
      inFlight.current = false
    }
  }

  return (
    <form onSubmit={submit} className="mt-5 space-y-4">
      <div className="space-y-2">
        <Label htmlFor="entry-label">Provider entry name</Label>
        <Input
          id="entry-label"
          value={label}
          onChange={(event) => setLabel(event.target.value)}
          required
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="entry-credential">API key</Label>
        <Input
          id="entry-credential"
          type="password"
          autoComplete="new-password"
          value={credential}
          onChange={(event) => setCredential(event.target.value)}
          required
        />
      </div>
      <div className="space-y-2">
        <Label htmlFor="entry-base-url">API endpoint</Label>
        <Input
          id="entry-base-url"
          type="url"
          value={baseUrl}
          onChange={(event) => setBaseUrl(event.target.value)}
          placeholder={
            capability.provider === 'openai_compatible'
              ? 'https://your-endpoint.example/v1'
              : capability.default_base_url ?? ''
          }
          required={capability.provider === 'openai_compatible'}
        />
      </div>
      {error ? (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      ) : null}
      <DialogFooter className="mt-6 gap-2">
        <Button type="button" variant="ghost" onClick={onBack}>
          Back
        </Button>
        <Button type="submit" disabled={create.isPending}>
          <ShieldCheck size={15} aria-hidden />
          {create.isPending ? 'Verifying…' : 'Add provider'}
        </Button>
      </DialogFooter>
    </form>
  )
}

/**
 * Four-step provider setup: choose a provider, choose how to authenticate,
 * connect, then verify the stored entry with a live connection test.
 */
function AddProviderWizard({
  open,
  onClose,
  onCreateAgent,
}: {
  open: boolean
  onClose: () => void
  onCreateAgent: (entryId: string | null) => void
}) {
  const providers = useAgentProviderCapabilitiesQuery()
  const [capability, setCapability] = useState<AgentProviderCapability | null>(null)
  const [method, setMethod] = useState<ProviderCredentialMethod | null>(null)
  const [connected, setConnected] = useState<{ id: string | null; label: string } | null>(null)

  useEffect(() => {
    if (!open) return
    setCapability(null)
    setMethod(null)
    setConnected(null)
  }, [open])

  const step: 1 | 2 | 3 | 4 = connected ? 4 : method ? 3 : capability ? 2 : 1

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <SectionKicker>
            {capability ? `${capability.display_name} · ` : ''}New provider · step {step} of 4
          </SectionKicker>
          <DialogTitle className="mt-1">
            {step === 1
              ? 'Choose a provider'
              : step === 2
                ? 'Choose how to authenticate'
                : step === 3
                  ? method === 'api_key'
                    ? 'Add an API-key entry'
                    : method === 'device_oauth'
                      ? 'Sign in with a device code'
                      : 'Continue in your browser'
                  : 'Provider connected'}
          </DialogTitle>
          <DialogDescription>
            {step === 1
              ? 'You can add the same provider more than once — for example two OpenAI accounts. Availability comes from the server capability catalog.'
              : step === 2
                ? 'Only the methods the server declares are offered. A guided login never replaces the API-key alternative.'
                : step === 3
                  ? 'A successful connection stores a protected credential and creates a provider entry — it does not create an agent. Secrets never return to this screen.'
                  : 'The credential is stored. Test the connection, then create an agent on this entry whenever you are ready.'}
          </DialogDescription>
        </DialogHeader>

        {step === 1 ? (
          <div className="mt-5 space-y-3">
            {providers.isLoading ? <LoadingPanel label="Loading provider catalog" /> : null}
            {providers.isError ? (
              <ErrorPanel
                title="Provider catalog unavailable"
                description="Forge could not load the authoritative credential-method catalog."
                onRetry={() => void providers.refetch()}
              />
            ) : null}
            <div className="max-h-[55vh] space-y-3 overflow-y-auto">
              {providers.data?.items.map((provider) => (
                <button
                  key={provider.provider}
                  type="button"
                  className="flex w-full items-center justify-between gap-3 rounded-md border border-border-subtle bg-card px-3 py-3 text-left transition-colors hover:border-ember-border"
                  onClick={() => setCapability(provider)}
                >
                  <div className="flex min-w-0 items-center gap-3">
                    <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-ember-surface text-primary">
                      <Key size={17} aria-hidden />
                    </div>
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium text-foreground">
                        {provider.display_name}
                      </p>
                      <p className="mt-0.5 font-mono text-micro uppercase tracking-[0.08em] text-muted-foreground">
                        {provider.model_discovery ? 'Model discovery' : 'Manual model selection'} ·{' '}
                        {provider.credential_methods.length} login method
                        {provider.credential_methods.length === 1 ? '' : 's'}
                      </p>
                    </div>
                  </div>
                  <CaretRight size={15} className="shrink-0 text-muted-foreground" aria-hidden />
                </button>
              ))}
            </div>
          </div>
        ) : null}

        {step === 2 && capability ? (
          <div className="mt-5 space-y-3">
            {capability.credential_methods.map((credential) => (
              <button
                key={credential.method}
                type="button"
                disabled={!credential.configured}
                className={`w-full rounded-md border px-3 py-3 text-left ${
                  credential.configured
                    ? 'border-border-subtle bg-card transition-colors hover:border-ember-border'
                    : 'cursor-not-allowed border-border-subtle bg-muted/40 opacity-70'
                }`}
                onClick={() => setMethod(credential.method)}
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="flex min-w-0 flex-wrap items-center gap-2">
                    <p className="text-sm font-medium text-foreground">
                      {credential.action_label}
                    </p>
                    <StateBadge
                      status={credential.support_level}
                      label={humanize(credential.support_level)}
                    />
                  </div>
                  {credential.configured ? (
                    <CaretRight size={15} className="shrink-0 text-muted-foreground" aria-hidden />
                  ) : null}
                </div>
                {credential.boundary_note ? (
                  <p className="mt-1.5 text-micro leading-5 text-muted-foreground">
                    {credential.boundary_note}
                  </p>
                ) : null}
                {credential.setup_guidance ? (
                  <p className="mt-1.5 text-micro leading-5 text-warning">
                    {credential.setup_guidance}
                  </p>
                ) : null}
              </button>
            ))}
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => setCapability(null)}>
                Back
              </Button>
            </DialogFooter>
          </div>
        ) : null}

        {step === 3 && capability && method ? (
          method === 'api_key' ? (
            <ApiKeyEntryForm
              capability={capability}
              onBack={() => setMethod(null)}
              onCreated={(entry) => setConnected({ id: entry.id, label: entry.label })}
            />
          ) : (
            <ProviderAuthorizationPanel
              key={`${capability.provider}:${method}`}
              capability={capability}
              method={method}
              onBack={() => setMethod(null)}
              onClose={onClose}
              onConnected={(entryId) =>
                setConnected({ id: entryId, label: capability.display_name })
              }
            />
          )
        ) : null}

        {step === 4 && connected ? (
          <div className="mt-5 space-y-4">
            <p
              className="flex items-start gap-2 rounded-md border border-success/30 bg-success/10 px-3 py-2.5 text-sm text-foreground"
              role="status"
            >
              <ShieldCheck size={16} className="mt-0.5 shrink-0 text-success" aria-hidden />
              <span>
                <strong>{connected.label}</strong> is connected. No agent was created.
              </span>
            </p>
            {connected.id ? (
              <ProviderConnectionTest entryId={connected.id} autoRun />
            ) : (
              <p className="text-xs text-muted-foreground">
                The entry is stored; run a connection test from its card on the Providers tab.
              </p>
            )}
            <DialogFooter className="gap-2">
              <Button type="button" variant="ghost" onClick={onClose}>
                Done
              </Button>
              <Button type="button" onClick={() => onCreateAgent(connected.id)}>
                Create an agent with this provider
              </Button>
            </DialogFooter>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

function ProviderEntryCard({
  entry,
  onShowAgents,
}: {
  entry: ProviderEntryResponse
  onShowAgents: () => void
}) {
  const rename = useRenameProviderEntryMutation()
  const remove = useRemoveProviderEntryMutation()
  const [renaming, setRenaming] = useState(false)
  const [confirmingRemoval, setConfirmingRemoval] = useState(false)
  const [label, setLabel] = useState(entry.label)
  const [error, setError] = useState<string>()
  const [notice, setNotice] = useState<string>()

  async function submitRename(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(undefined)
    try {
      await rename.mutateAsync({
        id: entry.id,
        input: { label: label.trim(), version: entry.version },
      })
      setRenaming(false)
    } catch (cause) {
      setError(
        isVersionConflict(cause)
          ? 'This entry changed in another session. Refresh before renaming.'
          : cause instanceof Error
            ? cause.message
            : 'Rename failed.',
      )
    }
  }

  async function disconnect() {
    setError(undefined)
    setNotice(undefined)
    try {
      const result = await remove.mutateAsync({ handleId: entry.id, version: entry.version })
      setConfirmingRemoval(false)
      setNotice(
        result.provider_revocation === 'failed'
          ? 'Disconnected locally. Provider-side revocation could not be confirmed; revoke Forge in the provider account as a follow-up.'
          : 'Provider entry disconnected. Referencing agents are now marked unhealthy.',
      )
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The entry could not be disconnected.')
    }
  }

  return (
    <Card className="flex flex-col p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate text-sm font-semibold text-foreground">
              {humanize(entry.provider)}
            </h3>
            <StateBadge status={entry.status} label={humanize(entry.status)} />
          </div>
          <p className="mt-1 truncate text-xs text-muted-foreground">{entry.label}</p>
        </div>
        <Key size={17} className="shrink-0 text-primary" aria-hidden />
      </div>
      <dl className="mt-3 space-y-1.5 text-xs">
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Method</dt>
          <dd className="text-foreground">
            {entry.credential_method === 'oauth_bundle' ? 'OAuth login' : 'API key'}
          </dd>
        </div>
        {entry.provider_account_id ? (
          <div className="flex justify-between gap-3">
            <dt className="text-muted-foreground">Account</dt>
            <dd className="truncate font-mono text-foreground">
              {shortId(entry.provider_account_id)}
            </dd>
          </div>
        ) : null}
        {entry.base_url ? (
          <div className="flex justify-between gap-3">
            <dt className="text-muted-foreground">Endpoint</dt>
            <dd className="truncate font-mono text-foreground">{entry.base_url}</dd>
          </div>
        ) : null}
        <div className="flex justify-between gap-3">
          <dt className="text-muted-foreground">Last used</dt>
          <dd className="text-foreground">
            {entry.last_used_at ? new Date(entry.last_used_at).toLocaleString() : 'Never'}
          </dd>
        </div>
      </dl>
      <button
        type="button"
        className="mt-3 inline-flex items-center gap-1.5 text-left text-xs font-medium text-primary hover:underline"
        onClick={onShowAgents}
      >
        Used by {entry.used_by.length} agent{entry.used_by.length === 1 ? '' : 's'}
        <ArrowUpRight size={13} aria-hidden />
      </button>
      {renaming ? (
        <form onSubmit={submitRename} className="mt-3 flex items-center gap-2">
          <Input
            aria-label="New entry name"
            value={label}
            onChange={(event) => setLabel(event.target.value)}
          />
          <Button type="submit" size="sm" disabled={rename.isPending}>
            Save
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={() => setRenaming(false)}>
            Cancel
          </Button>
        </form>
      ) : null}
      {confirmingRemoval ? (
        <div
          className="mt-3 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning"
          role="alertdialog"
          aria-label={`Confirm disconnecting ${entry.label}`}
        >
          {entry.used_by.length > 0 ? (
            <p>
              {entry.used_by.length} agent{entry.used_by.length === 1 ? '' : 's'} reference this
              entry ({entry.used_by.map((agent) => agent.agent_name).join(', ')}). They will become
              unhealthy and are never silently rebound.
            </p>
          ) : (
            <p>No agents reference this entry.</p>
          )}
          <div className="mt-2 flex gap-2">
            <Button size="sm" variant="destructive" disabled={remove.isPending} onClick={() => void disconnect()}>
              Disconnect
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setConfirmingRemoval(false)}>
              Keep
            </Button>
          </div>
        </div>
      ) : null}
      {error ? (
        <p role="alert" className="mt-2 text-xs text-destructive">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p role="status" className="mt-2 text-xs text-muted-foreground">
          {notice}
        </p>
      ) : null}
      {entry.status === 'configured' ? (
        <div className="mt-3">
          <ProviderConnectionTest entryId={entry.id} />
        </div>
      ) : null}
      {entry.status !== 'revoked' && !confirmingRemoval ? (
        <div className="mt-4 flex gap-2 border-t border-border-subtle pt-3">
          {!renaming ? (
            <Button size="sm" variant="outline" onClick={() => setRenaming(true)}>
              Rename
            </Button>
          ) : null}
          <Button size="sm" variant="outline" onClick={() => setConfirmingRemoval(true)}>
            Disconnect
          </Button>
        </div>
      ) : null}
    </Card>
  )
}

function CliRuntimeCard({ runtime }: { runtime: CliRuntimeEntryResponse }) {
  const authenticated = runtime.availability === 'authenticated'
  return (
    <Card className="flex flex-col p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate text-sm font-semibold text-foreground">
              {runtimeDisplayNames[runtime.kind] ?? humanize(runtime.kind)}
            </h3>
            <StateBadge
              status={authenticated ? 'healthy' : 'unavailable'}
              label={humanize(runtime.availability)}
            />
          </div>
          <p className="mt-1 truncate text-xs text-muted-foreground">
            {runtime.daemon_hostname ?? runtime.daemon_id} · {humanize(runtime.daemon_status)}
            {runtime.version ? ` · ${runtime.version}` : ''}
          </p>
        </div>
        <TerminalWindow size={17} className="shrink-0 text-primary" aria-hidden />
      </div>
      <p className="mt-3 text-xs text-muted-foreground">
        Used by {runtime.used_by.length} agent{runtime.used_by.length === 1 ? '' : 's'}
        {runtime.used_by.length > 0
          ? `: ${runtime.used_by.map((agent) => agent.agent_name).join(', ')}`
          : ''}
      </p>
      {!authenticated && runtime.login_hint ? (
        <p className="mt-2 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-warning">
          {runtime.login_hint}. Forge never reads the CLI&apos;s credential files.
        </p>
      ) : null}
    </Card>
  )
}

type WizardRuntime = { runtime: string; support_level: string; reason: string | null }

/** Three-step registration: authentication source → runtime → configure. */
function NewAgentDialog({
  open,
  onClose,
  entries,
  cliRuntimes,
  preselectedEntryId,
  onAddProvider,
}: {
  open: boolean
  onClose: () => void
  entries: ProviderEntryResponse[]
  cliRuntimes: CliRuntimeEntryResponse[]
  preselectedEntryId: string | null
  onAddProvider: () => void
}) {
  const capabilities = useAgentProviderCapabilitiesQuery()
  const createEmbedded = useCreateEmbeddedAgentMutation()
  const registerHarness = useRegisterHarnessAgentMutation()
  const [entryId, setEntryId] = useState<string | null>(preselectedEntryId)
  const [cliKind, setCliKind] = useState<string | null>(null)
  const [runtime, setRuntime] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [model, setModel] = useState('')
  const [systemPrompt, setSystemPrompt] = useState('')
  const [error, setError] = useState<string>()
  const inFlight = useRef(false)

  useEffect(() => {
    if (!open) return
    setEntryId(preselectedEntryId)
    setCliKind(null)
    setRuntime(null)
    setName('')
    setDescription('')
    setModel('')
    setSystemPrompt('')
    setError(undefined)
  }, [open, preselectedEntryId])

  const activeEntries = entries.filter((entry) => entry.status === 'configured')
  const selectedEntry = activeEntries.find((entry) => entry.id === entryId) ?? null
  const runtimeOptions: WizardRuntime[] = selectedEntry
    ? runtimeOptionsForEntry(capabilities.data?.items, selectedEntry)
    : cliKind
      ? [{ runtime: cliKind, support_level: 'stable', reason: null }]
      : []
  const capability = selectedEntry
    ? capabilities.data?.items.find((item) => item.provider === selectedEntry.provider)
    : undefined
  const step: 1 | 2 | 3 = !selectedEntry && !cliKind ? 1 : !runtime ? 2 : 3

  useEffect(() => {
    if (step === 3 && selectedEntry && !model) {
      setModel(capability?.default_model ?? '')
    }
  }, [capability?.default_model, model, selectedEntry, step])

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (inFlight.current || !runtime) return
    if (!name.trim()) {
      setError('A name is required.')
      return
    }
    inFlight.current = true
    setError(undefined)
    try {
      if (runtime === 'direct' && selectedEntry) {
        if (!model.trim()) {
          setError('A model is required for a direct agent.')
          return
        }
        await createEmbedded.mutateAsync({
          name: name.trim(),
          description: description.trim() ? description.trim() : null,
          credential_id: selectedEntry.id,
          model: model.trim(),
          system_prompt: systemPrompt.trim() ? systemPrompt.trim() : null,
          account_permission_ceiling: DEFAULT_CEILING,
          tool_policy: DEFAULT_CEILING,
        })
      } else {
        await registerHarness.mutateAsync({
          name: name.trim(),
          description: description.trim() ? description.trim() : null,
          executor_type: runtime,
          model: model.trim() ? model.trim() : null,
          credential_id: selectedEntry?.id ?? null,
        })
      }
      onClose()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The agent could not be created.')
    } finally {
      inFlight.current = false
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <SectionKicker>New agent · step {step} of 3</SectionKicker>
          <DialogTitle className="mt-1">
            {step === 1
              ? 'Choose an authentication source'
              : step === 2
                ? 'Choose a runtime'
                : 'Configure the agent'}
          </DialogTitle>
          <DialogDescription>
            {step === 1
              ? 'Pick the provider entry or CLI-managed runtime this agent authenticates with.'
              : step === 2
                ? 'Compatibility comes from the server capability catalog.'
                : 'Creation publishes an immutable profile. Bindings stay unchanged until you assign them.'}
          </DialogDescription>
        </DialogHeader>

        {step === 1 ? (
          <div className="mt-5 space-y-3">
            {activeEntries.length === 0 && cliRuntimes.length === 0 ? (
              <EmptyPanel
                title="No authentication sources"
                description="Add a provider first, or authenticate a CLI on a connected runtime."
                icon={<Key size={19} />}
              />
            ) : null}
            {activeEntries.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="flex w-full items-center justify-between gap-3 rounded-md border border-border-subtle bg-card px-3 py-2 text-left hover:border-ember-border"
                onClick={() => setEntryId(entry.id)}
              >
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium text-foreground">
                    {humanize(entry.provider)} · {entry.label}
                  </p>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {entry.credential_method === 'oauth_bundle' ? 'OAuth login' : 'API key'} · used
                    by {entry.used_by.length}
                  </p>
                </div>
                <CaretRight size={15} className="shrink-0 text-muted-foreground" aria-hidden />
              </button>
            ))}
            {cliRuntimes
              .filter(
                (runtimeEntry, index, all) =>
                  all.findIndex((candidate) => candidate.kind === runtimeEntry.kind) === index,
              )
              .map((runtimeEntry) => (
                <button
                  key={runtimeEntry.kind}
                  type="button"
                  className="flex w-full items-center justify-between gap-3 rounded-md border border-border-subtle bg-card px-3 py-2 text-left hover:border-ember-border"
                  onClick={() => setCliKind(runtimeEntry.kind)}
                >
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-foreground">
                      {runtimeDisplayNames[runtimeEntry.kind] ?? humanize(runtimeEntry.kind)}
                    </p>
                    <p className="mt-0.5 text-xs text-muted-foreground">
                      Uses its own CLI login · {humanize(runtimeEntry.availability)}
                    </p>
                  </div>
                  <CaretRight size={15} className="shrink-0 text-muted-foreground" aria-hidden />
                </button>
              ))}
            <DialogFooter className="gap-2">
              <Button type="button" variant="outline" onClick={onAddProvider}>
                <Plus size={15} aria-hidden />
                Add a provider
              </Button>
            </DialogFooter>
          </div>
        ) : null}

        {step === 2 ? (
          <div className="mt-5 space-y-3">
            <p className="text-xs text-muted-foreground">
              Source:{' '}
              <strong className="text-foreground">
                {selectedEntry
                  ? `${humanize(selectedEntry.provider)} · ${selectedEntry.label}`
                  : (runtimeDisplayNames[cliKind ?? ''] ?? humanize(cliKind))}
              </strong>
            </p>
            {runtimeOptions.map((option) => {
              const unavailable = option.support_level === 'unavailable'
              return (
                <button
                  key={option.runtime}
                  type="button"
                  disabled={unavailable}
                  className={`flex w-full items-center justify-between gap-3 rounded-md border px-3 py-2 text-left ${unavailable ? 'cursor-not-allowed border-border-subtle bg-muted/40 opacity-70' : 'border-border-subtle bg-card hover:border-ember-border'}`}
                  onClick={() => setRuntime(option.runtime)}
                >
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="text-sm font-medium text-foreground">
                        {runtimeDisplayNames[option.runtime] ?? humanize(option.runtime)}
                      </p>
                      <StateBadge
                        status={option.support_level}
                        label={humanize(option.support_level)}
                      />
                    </div>
                    {option.reason ? (
                      <p className="mt-0.5 text-xs text-muted-foreground">{option.reason}</p>
                    ) : null}
                  </div>
                  {!unavailable ? (
                    <CaretRight size={15} className="shrink-0 text-muted-foreground" aria-hidden />
                  ) : null}
                </button>
              )
            })}
            <DialogFooter>
              <Button
                type="button"
                variant="ghost"
                onClick={() => {
                  setEntryId(null)
                  setCliKind(null)
                }}
              >
                Back
              </Button>
            </DialogFooter>
          </div>
        ) : null}

        {step === 3 ? (
          <form onSubmit={submit} className="mt-5 space-y-4">
            <p className="text-xs text-muted-foreground">
              {selectedEntry
                ? `${humanize(selectedEntry.provider)} · ${selectedEntry.label}`
                : 'CLI-managed login'}{' '}
              → {runtimeDisplayNames[runtime ?? ''] ?? humanize(runtime)}
            </p>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="agent-name">Agent name</Label>
                <Input
                  id="agent-name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  placeholder="Forge assistant"
                  required
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="agent-model">Model{runtime === 'direct' ? '' : ' (optional)'}</Label>
                <Input
                  id="agent-model"
                  value={model}
                  onChange={(event) => setModel(event.target.value)}
                  required={runtime === 'direct'}
                />
              </div>
              <div className="space-y-2 sm:col-span-2">
                <Label htmlFor="agent-description">Description</Label>
                <Input
                  id="agent-description"
                  value={description}
                  onChange={(event) => setDescription(event.target.value)}
                  placeholder="What this agent is for"
                />
              </div>
              {runtime === 'direct' ? (
                <div className="space-y-2 sm:col-span-2">
                  <Label htmlFor="agent-prompt">System prompt (optional)</Label>
                  <Textarea
                    id="agent-prompt"
                    value={systemPrompt}
                    onChange={(event) => setSystemPrompt(event.target.value)}
                    placeholder="A bounded role for this agent"
                    rows={3}
                  />
                </div>
              ) : null}
            </div>
            {error ? (
              <p role="alert" className="text-xs text-destructive">
                {error}
              </p>
            ) : null}
            <DialogFooter className="gap-2">
              <Button type="button" variant="ghost" onClick={() => setRuntime(null)}>
                Back
              </Button>
              <Button
                type="submit"
                disabled={createEmbedded.isPending || registerHarness.isPending}
              >
                <ShieldCheck size={15} aria-hidden />
                {createEmbedded.isPending || registerHarness.isPending
                  ? 'Creating…'
                  : 'Create agent'}
              </Button>
            </DialogFooter>
          </form>
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

/** Publish a replacement profile for an existing embedded identity. */
function AddProfileDialog({
  identity,
  entries,
  onClose,
}: {
  identity: FederatedAgent | null
  entries: ProviderEntryResponse[]
  onClose: () => void
}) {
  const connectProfile = useConnectEmbeddedProfileMutation()
  const [entryId, setEntryId] = useState('')
  const [model, setModel] = useState('')
  const [error, setError] = useState<string>()
  const activeEntries = entries.filter((entry) => entry.status === 'configured')

  useEffect(() => {
    if (identity) {
      setEntryId(activeEntries[0]?.id ?? '')
      setModel(identity.model ?? '')
      setError(undefined)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [identity?.id])

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!identity) return
    if (!entryId || !model.trim()) {
      setError('A provider entry and model are required.')
      return
    }
    setError(undefined)
    try {
      await connectProfile.mutateAsync({
        identityId: identity.id,
        input: {
          version: identity.version,
          credential_id: entryId,
          model: model.trim(),
          permission_policy: 'scoped_proposals',
          tool_policy: DEFAULT_CEILING,
        },
      })
      onClose()
    } catch (cause) {
      setError(
        isVersionConflict(cause)
          ? 'The agent changed in another session. Refresh the roster and retry.'
          : cause instanceof Error
            ? cause.message
            : 'The profile could not be published.',
      )
    }
  }

  return (
    <Dialog open={Boolean(identity)} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-lg">
        <form onSubmit={submit}>
          <DialogHeader>
            <SectionKicker>Replace profile</SectionKicker>
            <DialogTitle className="mt-1">
              Publish a profile for {identity?.name ?? 'this agent'}
            </DialogTitle>
            <DialogDescription>
              The current profile stays active until verification and publication succeed.
            </DialogDescription>
          </DialogHeader>
          <div className="mt-5 space-y-4">
            <div className="space-y-2">
              <Label htmlFor="profile-entry">Provider entry</Label>
              <Select
                id="profile-entry"
                value={entryId}
                options={activeEntries.map((entry) => ({
                  value: entry.id,
                  label: `${humanize(entry.provider)} · ${entry.label}`,
                }))}
                onChange={setEntryId}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="profile-model">Model</Label>
              <Input
                id="profile-model"
                value={model}
                onChange={(event) => setModel(event.target.value)}
                required
              />
            </div>
            {error ? (
              <p role="alert" className="text-xs text-destructive">
                {error}
              </p>
            ) : null}
          </div>
          <DialogFooter className="mt-6 gap-2">
            <Button type="button" variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={connectProfile.isPending}>
              {connectProfile.isPending ? 'Verifying…' : 'Publish profile'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

type SettingsTab = 'providers' | 'agents' | 'bindings'

export function FederatedAgentsPage() {
  const routeSearch = useSearch({ strict: false }) as {
    project?: string
    provider?: string
    status?: string
    authorization?: string
    tab?: string
  }
  const agentsQuery = useFederatedAgentsQuery()
  const providersQuery = useProvidersQuery()
  const chatsQuery = useAgentChatsQuery()
  const [tab, setTab] = useState<SettingsTab>(() => {
    if (routeSearch.tab === 'providers' || routeSearch.status) return 'providers'
    if (routeSearch.tab === 'bindings' || routeSearch.project) return 'bindings'
    return 'agents'
  })
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [addProviderOpen, setAddProviderOpen] = useState(false)
  const [wizardOpen, setWizardOpen] = useState(false)
  const [wizardEntryId, setWizardEntryId] = useState<string | null>(null)
  const [profileIdentity, setProfileIdentity] = useState<FederatedAgent | null>(null)
  const [query, setQuery] = useState('')
  const [providerFilter, setProviderFilter] = useState('all')
  const [statusFilter, setStatusFilter] = useState('all')

  const agents = agentsQuery.data?.items ?? EMPTY_AGENTS
  const entries = providersQuery.data?.items ?? EMPTY_ENTRIES
  const cliRuntimes = providersQuery.data?.cli_runtimes ?? EMPTY_CLI_RUNTIMES

  const providerOptions = useMemo(
    () => [
      { value: 'all', label: 'All providers' },
      ...Array.from(new Set(agents.flatMap((agent) => (agent.provider ? [agent.provider] : []))))
        .sort()
        .map((provider) => ({ value: provider, label: humanize(provider) })),
    ],
    [agents],
  )
  const statusOptions = useMemo(
    () => [
      { value: 'all', label: 'All statuses' },
      ...Array.from(new Set(agents.map((agent) => agent.status)))
        .sort()
        .map((status) => ({ value: status, label: humanize(status) })),
    ],
    [agents],
  )
  const filteredAgents = useMemo(() => {
    const normalized = query.trim().toLowerCase()
    return agents.filter((agent) => {
      if (providerFilter !== 'all' && agent.provider !== providerFilter) return false
      if (statusFilter !== 'all' && agent.status !== statusFilter) return false
      if (!normalized) return true
      return [
        agent.name,
        agent.description,
        agent.executor_type,
        agent.provider,
        agent.model,
        agent.status,
      ]
        .filter(Boolean)
        .some((value) => String(value).toLowerCase().includes(normalized))
    })
  }, [agents, providerFilter, query, statusFilter])

  const chatEntries = chatsQuery.data?.items ?? EMPTY_CHAT_ENTRIES
  const tabs: { id: SettingsTab; label: string; count: number }[] = [
    { id: 'providers', label: 'Providers', count: entries.length },
    { id: 'agents', label: 'Agents', count: agents.length },
    { id: 'bindings', label: 'Bindings', count: chatEntries.length },
  ]

  return (
    <div className="min-h-full space-y-6 p-5 lg:p-8">
      <PageHeader
        eyebrow="Account-owned providers and agents"
        title="Agent Settings"
        description="Connect providers once, then create agents that use them directly or through a CLI harness."
        actions={
          tab === 'providers' ? (
            <Button onClick={() => setAddProviderOpen(true)}>
              <Plus size={16} aria-hidden />
              Add provider
            </Button>
          ) : tab === 'agents' ? (
            <Button
              onClick={() => {
                setWizardEntryId(null)
                setWizardOpen(true)
              }}
            >
              <Plus size={16} aria-hidden />
              New agent
            </Button>
          ) : null
        }
      />

      <div role="tablist" aria-label="Agent Settings sections" className="flex flex-wrap gap-x-6 gap-y-1 border-b border-border-subtle">
        {tabs.map((entry) => (
          <button
            key={entry.id}
            role="tab"
            id={`agent-settings-tab-${entry.id}`}
            aria-selected={tab === entry.id}
            aria-controls={`agent-settings-panel-${entry.id}`}
            className={`relative -mb-px inline-flex items-center gap-2 px-1 pb-3 pt-1 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
              tab === entry.id ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
            }`}
            onClick={() => setTab(entry.id)}
          >
            {entry.label}
            <span
              className={`rounded-full border px-1.5 py-px font-mono text-micro ${
                tab === entry.id
                  ? 'border-ember-border bg-ember-surface text-primary'
                  : 'border-border-subtle bg-muted text-muted-foreground'
              }`}
            >
              {entry.count}
            </span>
            <span
              aria-hidden
              className={`absolute inset-x-0 bottom-0 h-0.5 rounded-full transition-colors ${
                tab === entry.id ? 'bg-primary' : 'bg-transparent'
              }`}
            />
          </button>
        ))}
      </div>

      {tab === 'providers' ? (
        <div
          role="tabpanel"
          id="agent-settings-panel-providers"
          aria-labelledby="agent-settings-tab-providers"
          className="space-y-6"
        >
          {routeSearch.status ? (
            <div
              className="rounded-lg border border-ember-border bg-ember-surface px-4 py-3 text-sm text-foreground"
              role="status"
            >
              {routeSearch.provider ? humanize(routeSearch.provider) : 'Provider'} authorization{' '}
              <strong>{humanize(routeSearch.status)}</strong>.
              {routeSearch.status === 'succeeded' ? (
                <Button
                  size="sm"
                  variant="outline"
                  className="ml-3"
                  onClick={() => {
                    setWizardEntryId(null)
                    setTab('agents')
                    setWizardOpen(true)
                  }}
                >
                  Create an agent with this provider
                </Button>
              ) : null}
            </div>
          ) : null}
          {providersQuery.isLoading ? <LoadingPanel label="Loading provider entries" /> : null}
          {providersQuery.isError ? (
            <ErrorPanel
              title="Provider entries unavailable"
              description="Forge could not load the provider entry projection."
              onRetry={() => void providersQuery.refetch()}
            />
          ) : null}
          {!providersQuery.isLoading && !providersQuery.isError && entries.length === 0 ? (
            <EmptyPanel
              title="No providers connected"
              description="Add a provider to store its credential once, then create as many agents on it as you need."
              icon={<Key size={19} />}
            />
          ) : null}
          {entries.length > 0 ? (
            <section aria-labelledby="provider-entries-heading" className="space-y-3">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <SectionKicker>Connected providers</SectionKicker>
                  <h2
                    id="provider-entries-heading"
                    className="mt-1 text-lg font-semibold text-foreground"
                  >
                    Provider entries
                  </h2>
                  <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
                    Each entry is one credentialed connection. Add the same provider again for
                    another account or key.
                  </p>
                </div>
                <span className="inline-flex shrink-0 items-center gap-1.5 rounded-full border border-border-subtle bg-muted px-3 py-1 font-mono text-micro uppercase tracking-[0.8px] text-muted-foreground">
                  <Key size={13} aria-hidden />
                  Protected credentials only
                </span>
              </div>
              <div className="grid gap-3 lg:grid-cols-2 xl:grid-cols-3">
                {entries.map((entry) => (
                  <ProviderEntryCard
                    key={entry.id}
                    entry={entry}
                    onShowAgents={() => {
                      setQuery('')
                      setProviderFilter(
                        providerOptions.some((option) => option.value === entry.provider)
                          ? entry.provider
                          : 'all',
                      )
                      setTab('agents')
                    }}
                  />
                ))}
              </div>
            </section>
          ) : null}
          {cliRuntimes.length > 0 ? (
            <section aria-labelledby="cli-runtimes-heading" className="space-y-3">
              <div>
                <SectionKicker>CLI runtimes</SectionKicker>
                <h2 id="cli-runtimes-heading" className="mt-1 text-lg font-semibold text-foreground">
                  CLI-managed logins
                </h2>
                <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
                  Harnesses discovered on connected runtimes that manage their own authentication.
                  Forge reads availability only.
                </p>
              </div>
              <div className="grid gap-3 lg:grid-cols-2 xl:grid-cols-3">
                {cliRuntimes.map((runtime) => (
                  <CliRuntimeCard
                    key={`${runtime.daemon_id}:${runtime.kind}`}
                    runtime={runtime}
                  />
                ))}
              </div>
            </section>
          ) : null}
        </div>
      ) : tab === 'agents' ? (
        <div
          role="tabpanel"
          id="agent-settings-panel-agents"
          aria-labelledby="agent-settings-tab-agents"
          className="space-y-6"
        >
          {agentsQuery.isLoading ? <LoadingPanel label="Loading agent roster" /> : null}
          {agentsQuery.isError ? (
            <ErrorPanel
              title="Agent roster unavailable"
              onRetry={() => void agentsQuery.refetch()}
              description="The agent roster is unavailable. Existing Agent Chat history remains server-authoritative."
            />
          ) : null}
          {!agentsQuery.isLoading && !agentsQuery.isError && agents.length === 0 ? (
            <EmptyPanel
              title="No agents yet"
              description="1. Connect a provider. 2. Create an agent on it — directly or through a CLI harness."
              icon={<Robot size={19} />}
              action={
                <Button
                  onClick={() => {
                    setWizardEntryId(null)
                    setWizardOpen(true)
                  }}
                >
                  <Plus size={15} aria-hidden />
                  Get started
                </Button>
              }
            />
          ) : null}
          {!agentsQuery.isLoading && !agentsQuery.isError && agents.length > 0 ? (
            <section aria-labelledby="agent-roster-heading" className="space-y-3">
              <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
                <div>
                  <SectionKicker>Roster</SectionKicker>
                  <h2
                    id="agent-roster-heading"
                    className="mt-1 text-lg font-semibold text-foreground"
                  >
                    Agents
                  </h2>
                  <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
                    Every agent references one authentication source. Opening an agent does not
                    create a chat or grant it Project authority.
                  </p>
                </div>
                <div className="grid w-full gap-2 sm:w-auto sm:grid-cols-[minmax(12rem,18rem)_10rem_10rem]">
                  <label className="relative block">
                    <span className="sr-only">Search agents</span>
                    <MagnifyingGlass
                      size={15}
                      className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground"
                      aria-hidden
                    />
                    <Input
                      className="pl-9"
                      value={query}
                      onChange={(event) => setQuery(event.target.value)}
                      placeholder="Search agents"
                    />
                  </label>
                  <Select
                    value={providerFilter}
                    options={providerOptions}
                    onChange={setProviderFilter}
                    aria-label="Filter agents by provider"
                  />
                  <Select
                    value={statusFilter}
                    options={statusOptions}
                    onChange={setStatusFilter}
                    aria-label="Filter agents by status"
                  />
                </div>
              </div>
              <div className="grid gap-3 xl:grid-cols-2">
                {filteredAgents.map((agent) => (
                  <AgentCard
                    key={agent.id}
                    agent={agent}
                    entries={entries}
                    selected={agent.id === selectedId}
                    onSelect={() =>
                      setSelectedId((current) => (current === agent.id ? null : agent.id))
                    }
                    onAddProfile={() => setProfileIdentity(agent)}
                  />
                ))}
              </div>
              {filteredAgents.length === 0 ? (
                <EmptyPanel
                  title="No matching agents"
                  description="Try a provider, model, status, or agent name."
                  icon={<MagnifyingGlass size={19} />}
                />
              ) : null}
            </section>
          ) : null}
        </div>
      ) : agentsQuery.isError && chatsQuery.isError ? (
        <div
          role="tabpanel"
          id="agent-settings-panel-bindings"
          aria-labelledby="agent-settings-tab-bindings"
        >
          <ErrorPanel
            title="Agent bindings unavailable"
            description="Forge could not reach the server, so the Main and Project Agent bindings cannot load. Existing Agent Chat history remains server-authoritative."
            onRetry={() => {
              void agentsQuery.refetch()
              void chatsQuery.refetch()
            }}
          />
        </div>
      ) : (
        <div
          role="tabpanel"
          id="agent-settings-panel-bindings"
          aria-labelledby="agent-settings-tab-bindings"
          className="space-y-6"
        >
          <MainAgentBindingCard
            agents={agents}
            onConnect={() => {
              setWizardEntryId(null)
              setWizardOpen(true)
            }}
          />
          {routeSearch.project ? (
            <section
              className="rounded-lg border border-border-subtle bg-card p-4 sm:p-5"
              aria-label="Project Agent binding"
            >
              <ProjectAgentTab projectId={routeSearch.project} />
            </section>
          ) : null}
          <BoundAgentScopes
            entries={chatEntries}
            isLoading={chatsQuery.isLoading}
            isError={chatsQuery.isError}
            onRetry={() => void chatsQuery.refetch()}
          />
        </div>
      )}

      <AddProviderWizard
        open={addProviderOpen}
        onClose={() => setAddProviderOpen(false)}
        onCreateAgent={(entryId) => {
          setAddProviderOpen(false)
          setWizardEntryId(entryId)
          setTab('agents')
          setWizardOpen(true)
        }}
      />
      <NewAgentDialog
        open={wizardOpen}
        onClose={() => setWizardOpen(false)}
        entries={entries}
        cliRuntimes={cliRuntimes}
        preselectedEntryId={wizardEntryId}
        onAddProvider={() => {
          setWizardOpen(false)
          setTab('providers')
          setAddProviderOpen(true)
        }}
      />
      <AddProfileDialog
        identity={profileIdentity}
        entries={entries}
        onClose={() => setProfileIdentity(null)}
      />
    </div>
  )
}
