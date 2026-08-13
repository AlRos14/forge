import { useMemo, useState } from 'react'
import { Link } from '@tanstack/react-router'
import {
  ArrowUpRight,
  CaretRight,
  Key,
  Plus,
  Robot,
  ShieldCheck,
  Sparkle,
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
import { useAuthStore } from '@/stores/auth'
import { useAgentChatsQuery } from '@/features/agent-chat/hooks'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import {
  useAgentProfilesQuery,
  useAgentSessionsQuery,
  useConnectEmbeddedAgentMutation,
  useFederatedAgentsQuery,
  useSelectAgentProfileMutation,
  isVersionConflict,
} from '@/features/federation/hooks'
import type {
  AgentProfile,
  ConnectEmbeddedAgentInput,
  FederatedAgent,
} from '@/features/federation/types'
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

const initialForm: ConnectEmbeddedAgentInput = {
  name: '',
  description: '',
  provider: 'openai',
  base_url: 'https://api.openai.com/v1',
  model: 'gpt-4o-mini',
  credential_label: 'Primary key',
  credential: '',
  system_prompt: '',
  account_permission_ceiling: {
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
  },
  tool_policy: {
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
  },
  context_tokens: 32_000,
  max_input_tokens: 24_000,
  max_output_tokens: 8_000,
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
      className="overflow-hidden rounded-xl border border-ember-border bg-ember-surface shadow-ember"
    >
      <header className="border-b border-ember-border px-4 py-4 sm:px-5">
        <SectionKicker>Agent chat scopes</SectionKicker>
        <h2 id="bound-agent-scopes-heading" className="mt-1 text-lg font-semibold text-foreground">
          Main and Project Agent bindings
        </h2>
        <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
          These are the only durable chat owners. Task Workers and reviewers appear in Task detail,
          while connected but unbound identities remain in configuration below.
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
          No Main or Project Agent binding is visible yet. Connect an identity and choose its owning
          scope below.
        </p>
      )}
    </section>
  )
}

function AgentCard({
  agent,
  selected,
  onSelect,
}: {
  agent: FederatedAgent
  selected: boolean
  onSelect: () => void
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

  async function selectProfileVersion(profileId: string) {
    setProfileError(null)
    try {
      await selectProfile.mutateAsync({ profileId, version: agent.version })
    } catch (cause) {
      setProfileError(
        isVersionConflict(cause)
          ? 'Identity changed in another session. Refresh the roster before selecting a profile.'
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
            <StateBadge status={agent.effective_status ?? agent.status} />
          </div>
          <p className="mt-1 truncate font-mono text-xs text-muted-foreground">
            {agent.provider ?? agent.executor_type} · {agent.model ?? 'profile pending'}
          </p>
          <p className="mt-3 text-xs text-muted-foreground">
            {agent.backend_kind === 'native' ? 'Forge-hosted runtime' : 'External runtime'} ·{' '}
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
                <div className="flex justify-between gap-4">
                  <dt className="text-muted-foreground">Connection</dt>
                  <dd className="inline-flex items-center gap-1.5 text-foreground">
                    <StatusDot
                      status={activeSession?.connection_status ?? agent.effective_status}
                    />
                    {humanize(activeSession?.connection_status ?? agent.effective_status)}
                  </dd>
                </div>
              </dl>
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
                        <StateBadge status={session.status} />
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

function ConnectDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const [form, setForm] = useState(initialForm)
  const connect = useConnectEmbeddedAgentMutation()
  const [error, setError] = useState<string | null>(null)

  const update = <K extends keyof ConnectEmbeddedAgentInput>(
    key: K,
    value: ConnectEmbeddedAgentInput[K],
  ) => setForm((current) => ({ ...current, [key]: value }))

  async function submit(event: React.FormEvent) {
    event.preventDefault()
    setError(null)
    if (
      !form.name.trim() ||
      !form.credential.trim() ||
      !form.model.trim() ||
      !form.base_url.trim()
    ) {
      setError('Name, provider URL, model, and credential are required.')
      return
    }
    try {
      await connect.mutateAsync({
        ...form,
        name: form.name.trim(),
        credential: form.credential.trim(),
      })
      setForm(initialForm)
      onOpenChange(false)
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : 'Connection failed. Check the provider settings and retry.',
      )
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <form onSubmit={submit}>
          <DialogHeader>
            <SectionKicker>Embedded agent</SectionKicker>
            <DialogTitle className="mt-1">Connect a durable identity</DialogTitle>
            <DialogDescription>
              Credentials are stored as protected handles. This screen never renders the secret
              after connection.
            </DialogDescription>
          </DialogHeader>
          <div className="mt-6 grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="agent-name">Identity name</Label>
              <Input
                id="agent-name"
                value={form.name}
                onChange={(event) => update('name', event.target.value)}
                placeholder="Forge assistant"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="agent-provider">Provider</Label>
              <Select
                id="agent-provider"
                value={form.provider}
                options={[
                  { value: 'openai', label: 'OpenAI-compatible' },
                  { value: 'anthropic', label: 'Anthropic-compatible' },
                  { value: 'custom', label: 'Custom provider' },
                ]}
                onChange={(value) => update('provider', value)}
              />
            </div>
            <div className="space-y-2 sm:col-span-2">
              <Label htmlFor="agent-description">Description</Label>
              <Input
                id="agent-description"
                value={form.description ?? ''}
                onChange={(event) => update('description', event.target.value)}
                placeholder="What this identity is for"
              />
            </div>
            <div className="space-y-2 sm:col-span-2">
              <Label htmlFor="agent-base-url">Provider base URL</Label>
              <Input
                id="agent-base-url"
                type="url"
                value={form.base_url}
                onChange={(event) => update('base_url', event.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="agent-model">Model</Label>
              <Input
                id="agent-model"
                value={form.model}
                onChange={(event) => update('model', event.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="agent-credential-label">Credential label</Label>
              <Input
                id="agent-credential-label"
                value={form.credential_label}
                onChange={(event) => update('credential_label', event.target.value)}
              />
            </div>
            <div className="space-y-2 sm:col-span-2">
              <Label htmlFor="agent-credential">Credential</Label>
              <Input
                id="agent-credential"
                type="password"
                autoComplete="new-password"
                value={form.credential}
                onChange={(event) => update('credential', event.target.value)}
                placeholder="Stored in protected session storage"
              />
            </div>
            <div className="space-y-2 sm:col-span-2">
              <Label htmlFor="agent-prompt">System prompt (optional)</Label>
              <Textarea
                id="agent-prompt"
                value={form.system_prompt ?? ''}
                onChange={(event) => update('system_prompt', event.target.value)}
                placeholder="A bounded role for this identity"
                rows={3}
              />
            </div>
          </div>
          <div
            className="mt-6 rounded-lg border border-border-subtle bg-muted/20 px-4 py-3"
            role="note"
            aria-label="Initial permission ceiling"
          >
            <SectionKicker>Initial capability ceiling</SectionKicker>
            <p className="mt-2 text-xs leading-5 text-foreground">
              This connection starts with a reusable profile ceiling for later scoped admission:
            </p>
            <div className="mt-2 flex flex-wrap gap-1.5">
              {allowedPolicyValues(initialForm.tool_policy).map((capability) => (
                <span
                  key={capability}
                  className="rounded bg-muted px-2 py-1 font-mono text-micro text-foreground"
                >
                  {capability}
                </span>
              ))}
            </div>
            <p className="mt-2 text-micro leading-5 text-muted-foreground">
              Connecting an identity alone grants no Project, Agent Chat, Task, approval, or
              filesystem access. The server intersects this ceiling with the selected canonical
              scope before every session.
            </p>
          </div>
          {error ? (
            <p
              className="mt-4 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive"
              role="alert"
            >
              {error}
            </p>
          ) : null}
          <DialogFooter className="mt-6 gap-2">
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={connect.isPending}>
              <ShieldCheck size={15} aria-hidden />
              {connect.isPending ? 'Verifying…' : 'Connect identity'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function FederatedAgentsPage() {
  const user = useAuthStore((state) => state.user)
  const agentsQuery = useFederatedAgentsQuery()
  const chatsQuery = useAgentChatsQuery()
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [connectOpen, setConnectOpen] = useState(false)
  const agents = agentsQuery.data?.items ?? []
  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === selectedId),
    [agents, selectedId],
  )

  return (
    <div className="min-h-full space-y-6 p-5 lg:p-8">
      <PageHeader
        eyebrow="Federation"
        title="Agent identities"
        description="Durable identities with replaceable profiles, scoped sessions, and explicit connection health."
        actions={
          <Button onClick={() => setConnectOpen(true)}>
            <Plus size={16} aria-hidden />
            Connect agent
          </Button>
        }
      />
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-ember-border bg-ember-surface px-4 py-3">
        <div className="flex items-center gap-3">
          <Sparkle size={17} className="text-primary" aria-hidden />
          <p className="text-sm text-foreground">
            {user?.display_name ?? user?.email ?? 'Your account'} owns these identities.
          </p>
        </div>
        <div className="flex items-center gap-2 font-mono text-micro uppercase tracking-[0.8px] text-muted-foreground">
          <Key size={13} aria-hidden />
          Protected credentials only
        </div>
      </div>
      <MainAgentBindingCard agents={agents} onConnect={() => setConnectOpen(true)} />
      <BoundAgentScopes
        entries={chatsQuery.data?.items ?? []}
        isLoading={chatsQuery.isLoading}
        isError={chatsQuery.isError}
        onRetry={() => void chatsQuery.refetch()}
      />
      {agentsQuery.isLoading ? <LoadingPanel label="Loading identity roster" /> : null}
      {agentsQuery.isError ? (
        <ErrorPanel
          onRetry={() => void agentsQuery.refetch()}
          description="The identity roster is unavailable. Existing Agent Chat history remains server-authoritative."
        />
      ) : null}
      {!agentsQuery.isLoading && !agentsQuery.isError && agents.length === 0 ? (
        <EmptyPanel
          title="No embedded identities"
          description="Connect a provider-backed identity to bind it to the Main Agent Chat, a Project Agent Chat, or a scoped Task session."
          icon={<Robot size={19} />}
        />
      ) : null}
      {!agentsQuery.isLoading && !agentsQuery.isError && agents.length > 0 ? (
        <section aria-labelledby="identity-configuration-heading" className="space-y-3">
          <div>
            <SectionKicker>Configuration</SectionKicker>
            <h2
              id="identity-configuration-heading"
              className="mt-1 text-lg font-semibold text-foreground"
            >
              Connected identity inventory
            </h2>
            <p className="mt-1 max-w-2xl text-sm leading-6 text-muted-foreground">
              Profiles, connection health, and unbound identities live here. Opening an identity
              does not create a chat or grant it Project authority.
            </p>
          </div>
          <div className="grid gap-3 xl:grid-cols-2">
            {agents.map((agent) => (
              <AgentCard
                key={agent.id}
                agent={agent}
                selected={agent.id === selectedId}
                onSelect={() =>
                  setSelectedId((current) => (current === agent.id ? null : agent.id))
                }
              />
            ))}
          </div>
        </section>
      ) : null}
      <ConnectDialog open={connectOpen} onOpenChange={setConnectOpen} />
      {selectedAgent ? (
        <p className="sr-only" aria-live="polite">
          Selected {selectedAgent.name}
        </p>
      ) : null}
    </div>
  )
}

export function FederatedAgentProfileSummary({ profile }: { profile: AgentProfile }) {
  return (
    <span>
      {profile.provider ?? profile.executor_type} · {profile.model ?? 'unknown'}
    </span>
  )
}
