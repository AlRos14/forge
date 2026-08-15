import type { ReactNode } from 'react'
import { Link } from '@tanstack/react-router'
import {
  ArrowClockwise,
  ArrowUpRight,
  Brain,
  CheckCircle,
  Clock,
  Gauge,
  Pulse,
  Question,
  WarningCircle,
} from '@phosphor-icons/react'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useAgentChatsQuery } from '@/features/agent-chat/hooks'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import { useMissionControlQuery } from '@/features/federation/hooks'
import type {
  AgentHealthItem,
  AttentionConsumerHealth,
  AttentionItem,
  MissionControlResponse,
  MissionControlWorkItem,
  OutcomeItem,
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

function formatDate(value: string | null | undefined): string {
  if (!value) return 'No date'
  const date = new Date(value)
  return Number.isNaN(date.getTime())
    ? value
    : date.toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })
}

function humanize(value: string | null | undefined): string {
  if (!value) return 'Unknown'
  return value.replaceAll('_', ' ').replace(/\b\w/g, (letter) => letter.toUpperCase())
}

function count(value: bigint | number): number {
  return typeof value === 'bigint' ? Number(value) : value
}

function attentionTone(item: AttentionItem): string {
  if (
    ['validation_failed', 'run_stalled', 'retry_exhausted', 'runtime_offline'].includes(
      item.category,
    )
  ) {
    return 'border-destructive/30 bg-destructive/5'
  }
  if (
    ['human_input_required', 'review_risk', 'budget_threshold', 'commitment_overdue'].includes(
      item.category,
    )
  ) {
    return 'border-warning/30 bg-warning/5'
  }
  return 'border-border-subtle bg-card'
}

function AttentionCard({ item }: { item: AttentionItem }) {
  return (
    <article className={`rounded-lg border p-4 ${attentionTone(item)}`}>
      <div className="flex items-start gap-3">
        <WarningCircle size={18} className="mt-0.5 shrink-0 text-warning" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="text-sm font-semibold text-foreground">{item.summary}</h3>
            <StateBadge status={item.lifecycle} />
          </div>
          <p className="mt-1 text-xs leading-5 text-muted-foreground">{humanize(item.category)}</p>
          <p className="mt-3 font-mono text-micro text-muted-foreground">
            {humanize(item.scope_type)} · {item.scope_id.slice(0, 8)} · priority {item.priority} ·
            event {item.source_event_id.slice(0, 8)}
          </p>
          {item.recommended_action ? (
            <p className="mt-2 text-xs font-medium text-foreground">
              Next: {item.recommended_action}
            </p>
          ) : null}
        </div>
      </div>
    </article>
  )
}

function AgentHealthCard({ item, scopeLabel }: { item: AgentHealthItem; scopeLabel: string }) {
  const status = item.connection_status ?? item.identity_status
  return (
    <article className="rounded-lg border border-border-subtle bg-card p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <StatusDot status={status} />
          <h3 className="truncate text-sm font-semibold text-foreground">{item.name}</h3>
        </div>
        <StateBadge status={status} />
      </div>
      <p className="mt-2 text-xs font-medium text-primary">{scopeLabel}</p>
      <div className="mt-3 grid gap-2 text-xs sm:grid-cols-2">
        <div>
          <p className="text-muted-foreground">Provider</p>
          <p className="mt-1 truncate font-mono text-foreground">
            {item.provider ?? item.backend_kind ?? 'Native'}
          </p>
        </div>
        <div>
          <p className="text-muted-foreground">Model</p>
          <p className="mt-1 truncate font-mono text-foreground">
            {item.model ?? 'Profile pending'}
          </p>
        </div>
        <div>
          <p className="text-muted-foreground">Sessions</p>
          <p className="mt-1 font-mono text-foreground">{item.active_session_count} active</p>
        </div>
        <div>
          <p className="text-muted-foreground">Project scopes</p>
          <p className="mt-1 font-mono text-foreground">{item.project_count}</p>
        </div>
      </div>
      <p className="mt-3 border-t border-border-subtle pt-2 font-mono text-micro text-muted-foreground">
        {item.paused ? 'Paused' : humanize(item.identity_status)} · last activity{' '}
        {formatDate(item.last_activity_at)}
      </p>
    </article>
  )
}

function BindingScopeRow({ entry }: { entry: AgentChatEntry }) {
  const isMain = entry.kind === 'main'
  const to = isMain ? '/chat' : '/projects/$projectId/chat'
  const label = isMain ? 'Global · Main' : (entry.project_name ?? 'Project Agent')
  const identity = entry.identity_name ?? 'No identity selected'
  const status = entry.binding_state === 'active' ? entry.chat_status : entry.binding_state

  return (
    <Link
      to={to}
      params={isMain ? undefined : { projectId: entry.project_id ?? '' }}
      className="flex min-w-0 items-center justify-between gap-3 px-4 py-3 transition-colors hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-foreground">{label}</p>
        <p className="mt-1 truncate text-xs text-muted-foreground">
          {identity} · {isMain ? 'account-owned timeline' : 'Project-owned timeline'}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        {count(entry.pending_turn_count) > 0 ? (
          <span className="font-mono text-micro text-muted-foreground">
            {count(entry.pending_turn_count)} pending
          </span>
        ) : null}
        <StateBadge status={status} label={humanize(status)} />
        <ArrowUpRight size={15} className="text-muted-foreground" aria-hidden />
      </div>
    </Link>
  )
}

function BindingScopes({
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
  if (isLoading) {
    return (
      <Card className="border-border-subtle bg-card p-4" role="status" aria-live="polite">
        Loading Main and Project Agent bindings…
      </Card>
    )
  }
  if (isError) {
    return (
      <ErrorPanel
        title="Agent binding projection unavailable"
        description="Mission Control could not load the current Main and Project Agent scopes. Retry before relying on the roster."
        onRetry={onRetry}
      />
    )
  }
  if (entries.length === 0) {
    return (
      <EmptyPanel
        title="No bound Agent Chat scopes"
        description="Connect and bind a Main or Project Agent to make its durable timeline visible here. Unbound identities remain in Agent settings."
        icon={<Brain size={19} />}
      />
    )
  }
  return (
    <ProjectionSection
      title="Main and Project Agent bindings"
      count={entries.length}
      icon={<Brain size={16} />}
    >
      <div className="divide-y divide-border-subtle">
        {entries.map((entry) => (
          <BindingScopeRow key={entry.chat_id} entry={entry} />
        ))}
      </div>
    </ProjectionSection>
  )
}

function WorkRow({ item }: { item: MissionControlWorkItem }) {
  return (
    <Link
      to="/tasks/$taskId"
      params={{ taskId: item.task_id }}
      className="flex items-center justify-between gap-3 px-4 py-3 transition-colors hover:bg-muted/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
    >
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-foreground">{item.title}</p>
        <p className="mt-1 text-xs text-muted-foreground">
          Project {item.project_id.slice(0, 8)} · {item.primary_action}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <StateBadge status={item.status} />
        <ArrowUpRight size={15} className="text-muted-foreground" aria-hidden />
      </div>
    </Link>
  )
}

function OutcomeRow({ item }: { item: OutcomeItem }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border-subtle px-4 py-3 last:border-b-0">
      <div className="min-w-0">
        <p className="truncate text-sm font-medium text-foreground">{item.title}</p>
        <p className="mt-1 text-xs text-muted-foreground">
          Project {item.project_id.slice(0, 8)} · {item.outcome}
        </p>
      </div>
      <span className="font-mono text-micro text-muted-foreground">
        {formatDate(item.occurred_at)}
      </span>
    </div>
  )
}

function Capacity({ data }: { data: MissionControlResponse }) {
  const capacity = data.capacity
  return (
    <Card className="border-border-subtle bg-card p-4">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Gauge size={17} className="text-primary" aria-hidden />
          <SectionKicker>Runtime capacity</SectionKicker>
        </div>
        <StateBadge
          status={capacity.healthy ? 'healthy' : 'attention'}
          label={capacity.healthy ? 'Healthy' : 'Attention'}
        />
      </div>
      <div className="mt-3 grid grid-cols-3 gap-3">
        <div>
          <p className="font-mono text-xl font-semibold tabular-nums text-foreground">
            {capacity.active_executions}
          </p>
          <p className="text-micro text-muted-foreground">executions</p>
        </div>
        <div>
          <p className="font-mono text-xl font-semibold tabular-nums text-foreground">
            {capacity.queued_tasks}
          </p>
          <p className="text-micro text-muted-foreground">queued</p>
        </div>
        <div>
          <p className="font-mono text-xl font-semibold tabular-nums text-foreground">
            {capacity.active_sessions}
          </p>
          <p className="text-micro text-muted-foreground">sessions</p>
        </div>
      </div>
    </Card>
  )
}

function ConsumerHealth({ health }: { health: AttentionConsumerHealth | null }) {
  if (!health)
    return (
      <Card className="border-border-subtle bg-card p-4">
        <div className="flex items-center gap-2">
          <Pulse size={17} className="text-muted-foreground" aria-hidden />
          <SectionKicker>Projection consumer</SectionKicker>
        </div>
        <p className="mt-3 text-xs text-muted-foreground">
          No consumer health projection returned.
        </p>
      </Card>
    )
  const status = health.stale || health.last_error_code ? 'attention' : 'healthy'
  const label = health.stale ? 'Stale' : health.last_error_code ? 'Attention' : 'Healthy'
  return (
    <Card className="border-border-subtle bg-card p-4">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Pulse size={17} className="text-primary" aria-hidden />
          <SectionKicker>Projection consumer</SectionKicker>
        </div>
        <StateBadge status={status} label={label} />
      </div>
      <p className="mt-3 text-sm font-medium text-foreground">{health.consumer_name}</p>
      <div className="mt-2 grid gap-2 text-xs sm:grid-cols-2">
        <div>
          <p className="text-muted-foreground">Processed events</p>
          <p className="mt-1 font-mono text-foreground">{health.processed_events}</p>
        </div>
        <div>
          <p className="text-muted-foreground">Last sequence</p>
          <p className="mt-1 font-mono text-foreground">{health.last_sequence}</p>
        </div>
      </div>
      <p className="mt-3 border-t border-border-subtle pt-2 font-mono text-micro text-muted-foreground">
        Last success {formatDate(health.last_success_at)}
        {health.last_error_code ? ` · ${health.last_error_code}` : ''}
      </p>
    </Card>
  )
}

function ProjectionSection({
  title,
  count,
  children,
  icon,
}: {
  title: string
  count: number
  children: ReactNode
  icon: ReactNode
}) {
  return (
    <section className="overflow-hidden rounded-xl border border-border-subtle bg-card shadow-card">
      <header className="flex items-center justify-between gap-3 border-b border-border-subtle px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="text-primary">{icon}</span>
          <h2 className="font-mono text-micro font-semibold uppercase tracking-[0.9px] text-muted-foreground">
            {title}
          </h2>
        </div>
        <span className="rounded-full bg-muted px-2 py-0.5 font-mono text-micro text-muted-foreground">
          {count}
        </span>
      </header>
      {children}
    </section>
  )
}

function MissionContent({
  data,
  chatEntries,
  chatIsLoading,
  chatIsError,
  onRetryChats,
}: {
  data: MissionControlResponse
  chatEntries: AgentChatEntry[]
  chatIsLoading: boolean
  chatIsError: boolean
  onRetryChats: () => void
}) {
  const attention = data.needs_attention
  const humanInput = attention.filter((item) => item.category === 'human_input_required')
  const commitments = attention.filter((item) => item.category === 'commitment_overdue')
  const otherAttention = attention.filter(
    (item) => item.category !== 'human_input_required' && item.category !== 'commitment_overdue',
  )
  const boundIdentityIds = new Set(
    chatEntries.flatMap((entry) => (entry.identity_id ? [entry.identity_id] : [])),
  )
  const relevantAgentHealth = data.agent_health.filter(
    (item) => boundIdentityIds.has(item.identity_id) || item.active_session_count > 0,
  )
  const scopeForIdentity = new Map(
    chatEntries
      .filter((entry) => entry.identity_id)
      .map((entry) => [
        entry.identity_id as string,
        entry.kind === 'main' ? 'Main Agent' : `${entry.project_name ?? 'Project'} Agent`,
      ]),
  )
  const hasAny =
    attention.length +
      data.review_ready.length +
      data.active_work.length +
      relevantAgentHealth.length +
      data.recent_outcomes.length >
    0

  return (
    <div className="space-y-5">
      <BindingScopes
        entries={chatEntries}
        isLoading={chatIsLoading}
        isError={chatIsError}
        onRetry={onRetryChats}
      />
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        <Card className="border-border-subtle bg-card p-4">
          <SectionKicker>Needs attention</SectionKicker>
          <p className="mt-2 font-mono text-2xl font-semibold text-foreground">
            {attention.length}
          </p>
          <p className="mt-1 text-xs text-muted-foreground">
            Server-authored actionable conditions
          </p>
        </Card>
        <Card className="border-border-subtle bg-card p-4">
          <SectionKicker>Review ready</SectionKicker>
          <p className="mt-2 font-mono text-2xl font-semibold text-foreground">
            {data.review_ready.length}
          </p>
          <p className="mt-1 text-xs text-muted-foreground">Work awaiting review</p>
        </Card>
        <Card className="border-border-subtle bg-card p-4">
          <SectionKicker>Active work</SectionKicker>
          <p className="mt-2 font-mono text-2xl font-semibold text-foreground">
            {data.active_work.length}
          </p>
          <p className="mt-1 text-xs text-muted-foreground">Current assigned execution</p>
        </Card>
        <Capacity data={data} />
      </div>
      <div className="grid gap-5 xl:grid-cols-2">
        <ConsumerHealth health={data.consumer_health} />
        {relevantAgentHealth.length > 0 ? (
          <ProjectionSection
            title="Relevant Task agent health"
            count={relevantAgentHealth.length}
            icon={<Pulse size={16} />}
          >
            <div className="grid gap-2 p-3 md:grid-cols-2">
              {relevantAgentHealth.map((item) => (
                <AgentHealthCard
                  key={item.identity_id}
                  item={item}
                  scopeLabel={scopeForIdentity.get(item.identity_id) ?? 'Relevant Task agent'}
                />
              ))}
            </div>
          </ProjectionSection>
        ) : null}
      </div>
      {!hasAny ? (
        <EmptyPanel
          title="All scopes are quiet"
          description="Mission Control will surface attention, review-ready work, and agent health as durable projections change."
          icon={<CheckCircle size={19} />}
        />
      ) : null}
      {otherAttention.length > 0 ? (
        <ProjectionSection
          title="Needs attention"
          count={otherAttention.length}
          icon={<WarningCircle size={16} />}
        >
          <div className="space-y-2 p-3">
            {otherAttention.map((item) => (
              <AttentionCard key={item.id} item={item} />
            ))}
          </div>
        </ProjectionSection>
      ) : null}
      {humanInput.length > 0 ? (
        <ProjectionSection
          title="Questions / human input"
          count={humanInput.length}
          icon={<Question size={16} />}
        >
          <div className="space-y-2 p-3">
            {humanInput.map((item) => (
              <AttentionCard key={item.id} item={item} />
            ))}
          </div>
        </ProjectionSection>
      ) : null}
      {commitments.length > 0 ? (
        <ProjectionSection
          title="Commitment alerts"
          count={commitments.length}
          icon={<Clock size={16} />}
        >
          <div className="space-y-2 p-3">
            {commitments.map((item) => (
              <AttentionCard key={item.id} item={item} />
            ))}
          </div>
        </ProjectionSection>
      ) : null}
      {data.review_ready.length > 0 ? (
        <ProjectionSection
          title="Review-ready work"
          count={data.review_ready.length}
          icon={<CheckCircle size={16} />}
        >
          <div className="divide-y divide-border-subtle">
            {data.review_ready.map((item) => (
              <WorkRow key={item.task_id} item={item} />
            ))}
          </div>
        </ProjectionSection>
      ) : null}
      {data.active_work.length > 0 ? (
        <ProjectionSection
          title="Active work"
          count={data.active_work.length}
          icon={<Pulse size={16} />}
        >
          <div className="divide-y divide-border-subtle">
            {data.active_work.map((item) => (
              <WorkRow key={item.task_id} item={item} />
            ))}
          </div>
        </ProjectionSection>
      ) : null}
      {data.recent_outcomes.length > 0 ? (
        <ProjectionSection
          title="Recent outcomes"
          count={data.recent_outcomes.length}
          icon={<CheckCircle size={16} />}
        >
          <div>
            {data.recent_outcomes.map((item) => (
              <OutcomeRow key={`${item.task_id}:${item.occurred_at}`} item={item} />
            ))}
          </div>
        </ProjectionSection>
      ) : null}
    </div>
  )
}

export function MissionControlPage() {
  const query = useMissionControlQuery()
  const chatsQuery = useAgentChatsQuery()
  const computedAt = query.data?.computed_at ? Date.parse(query.data.computed_at) : Number.NaN
  const isStale = Boolean(
    query.data &&
    (query.data.consumer_health?.stale ||
      !Number.isFinite(computedAt) ||
      Date.now() - computedAt > 60_000),
  )
  return (
    <div className="min-h-full space-y-6 p-5 lg:p-8">
      <PageHeader
        eyebrow="Mission Control"
        title="What needs your attention?"
        description="A read-only operational projection across authorized account and project scopes."
        actions={
          <>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                void query.refetch()
                void chatsQuery.refetch()
              }}
              disabled={query.isFetching || chatsQuery.isFetching}
            >
              <ArrowClockwise
                size={14}
                className={query.isFetching || chatsQuery.isFetching ? 'animate-spin' : ''}
                aria-hidden
              />
              Refresh
            </Button>
            <Link
              to="/agents"
              className="inline-flex h-8 items-center gap-1.5 rounded-md border border-input bg-card px-3 text-xs font-medium text-foreground transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <Brain size={14} aria-hidden />
              Agent settings
            </Link>
          </>
        }
      />
      {isStale ? (
        <div
          className="flex items-center gap-2 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-foreground"
          role="status"
        >
          <WarningCircle size={15} className="text-warning" aria-hidden />
          Projection is stale. Refreshing from authoritative records may take a moment.
        </div>
      ) : null}
      {query.isLoading ? <LoadingPanel label="Loading Mission Control projection" /> : null}
      {query.isError ? (
        <ErrorPanel
          title="Mission Control projection unavailable"
          description="The attention read model is unavailable. No client-side state is synthesized from raw events."
          onRetry={() => void query.refetch()}
        />
      ) : null}
      {query.data ? (
        <MissionContent
          data={query.data}
          chatEntries={chatsQuery.data?.items ?? []}
          chatIsLoading={chatsQuery.isLoading}
          chatIsError={chatsQuery.isError}
          onRetryChats={() => void chatsQuery.refetch()}
        />
      ) : null}
      {query.data ? (
        <p className="flex items-center gap-2 font-mono text-micro text-muted-foreground">
          <Clock size={12} aria-hidden />
          Computed {formatDate(query.data.computed_at)} · projections refresh from committed events
        </p>
      ) : null}
    </div>
  )
}
