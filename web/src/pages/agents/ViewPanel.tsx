import { Copy, Pause, PencilSimple, Play } from '@phosphor-icons/react'
import { Link } from '@tanstack/react-router'
import { useAgentRecentTasks } from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { TaskStatusBadge } from '@/components/task-controls'
import { Avatar } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import type { Agent, AgentStatus, Daemon } from '@/types/generated'
import { effectiveStatusLabels, executorDisplayNames, statusConfig } from './constants'
import { formatDate, formatDuration, formatPercent } from './form-utils'
import { InfoField } from './shared'

export function ViewPanel({
  agent,
  daemons,
  showDaemonDetails,
  onEdit,
  onDuplicate,
  onTogglePaused,
  duplicatePending,
  pausePending,
}: {
  agent: Agent
  daemons: Daemon[]
  showDaemonDetails: boolean
  onEdit: () => void
  onDuplicate: () => void
  onTogglePaused: () => void
  duplicatePending: boolean
  pausePending: boolean
}) {
  const status = statusConfig[agent.status as AgentStatus] ?? statusConfig.offline
  const effective = agent.effective_status ?? undefined
  const hasConfig = Object.keys(agent.config_json ?? {}).length > 0
  const configStr = hasConfig ? JSON.stringify(agent.config_json, null, 2) : ''
  const pinnedDaemon = agent.daemon_id ? (daemons.find((d) => d.id === agent.daemon_id) ?? null) : null
  const active = agent.active_task_count ?? 0
  const capacity = agent.max_concurrent_tasks
  const recentTasksQuery = useAgentRecentTasks(agent.id, 5)
  const recentTasks = recentTasksQuery.data ?? []

  const configRows: Array<{ key: string; value: string; mono?: boolean }> = [
    { key: 'executor', value: executorDisplayNames[agent.executor_type] ?? agent.executor_type, mono: true },
    { key: 'model', value: agent.model || 'default', mono: true },
    ...(showDaemonDetails
      ? pinnedDaemon
        ? [{ key: 'daemon', value: pinnedDaemon.hostname || pinnedDaemon.machine_id, mono: true }]
        : agent.daemon_id
          ? [{ key: 'daemon', value: agent.daemon_id, mono: true }]
          : []
      : []),
    ...(agent.permission_policy ? [{ key: 'policy', value: agent.permission_policy, mono: true }] : []),
    ...(agent.reasoning_effort ? [{ key: 'reasoning', value: agent.reasoning_effort, mono: true }] : []),
    { key: 'system prompt', value: agent.prompt_template ? 'set' : 'not set', mono: true },
    { key: 'capabilities', value: agent.capabilities.length > 0 ? agent.capabilities.join(', ') : 'none', mono: true },
  ]

  return (
    <div className="flex flex-1 flex-col overflow-y-auto">
      <header className="flex shrink-0 items-center gap-3.5 px-6 py-5">
        <Avatar name={agent.name} seed={agent.id} size="lg" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h2 className="truncate text-lg font-semibold">{agent.name}</h2>
            {effective && effective !== 'active' && effective !== 'idle' && effective !== 'busy' ? (
              <span className="shrink-0 text-sm text-amber-600 dark:text-amber-400">
                {effectiveStatusLabels[effective] ?? effective}
              </span>
            ) : null}
          </div>
          <p className="mt-0.5 truncate font-mono text-[12px] text-muted-foreground">
            {executorDisplayNames[agent.executor_type] ?? agent.executor_type}
            {showDaemonDetails
              ? pinnedDaemon
                ? ` · ${pinnedDaemon.hostname || pinnedDaemon.machine_id}`
                : agent.daemon_id
                  ? ` · ${agent.daemon_id}`
                  : ''
              : ''}
          </p>
          {agent.description ? (
            <p className="mt-1 text-[12px] text-muted-foreground">{agent.description}</p>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          <span
            className={cn(
              'rounded-full px-3 py-1 text-[12px] font-medium',
              agent.status === 'idle'
                ? 'bg-orange-500/12 text-orange-400'
                : agent.status === 'busy'
                  ? 'bg-amber-400/12 text-amber-400'
                  : agent.status === 'error'
                    ? 'bg-red-500/12 text-red-400'
                    : 'bg-stone-500/15 text-stone-400',
            )}
          >
            {status.label}
          </span>
          {agent.paused ? (
            <Badge variant="outline" className="text-micro uppercase">
              Paused
            </Badge>
          ) : null}
          <Button size="sm" variant="outline" onClick={onTogglePaused} disabled={pausePending}>
            {agent.paused ? <Play size={14} className="mr-1.5" /> : <Pause size={14} className="mr-1.5" />}
            {pausePending ? 'Saving...' : agent.paused ? 'Resume' : 'Pause'}
          </Button>
          <Button size="sm" variant="outline" onClick={onDuplicate} disabled={duplicatePending}>
            <Copy size={14} className="mr-1.5" />
            {duplicatePending ? 'Copying...' : 'Duplicate'}
          </Button>
          <Button size="sm" variant="outline" onClick={onEdit}>
            <PencilSimple size={14} className="mr-1.5" />
            Edit
          </Button>
        </div>
      </header>

      <div className="flex-1 space-y-6 px-6 py-5">
        <div className="grid grid-cols-4 gap-2.5">
          {[
            { label: 'Tasks', value: `${active}/${capacity}` },
            { label: 'Total runs', value: agent.total_runs ?? 0 },
            { label: 'Avg duration', value: formatDuration(agent.avg_duration_ms) },
            { label: 'Success rate', value: formatPercent(agent.success_rate) },
          ].map((stat) => (
            <div key={stat.label} className="rounded-lg border border-border-subtle bg-muted px-3.5 py-3">
              <p className="mb-2 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
                {stat.label}
              </p>
              <p className="font-mono text-xl font-semibold tabular-nums text-foreground">{stat.value}</p>
            </div>
          ))}
        </div>

        <section>
          <h3 className="mb-2.5 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
            Configuration
          </h3>
          <div className="overflow-hidden rounded-lg border border-border-subtle bg-muted">
            {configRows.map((row, i) => (
              <div
                key={row.key}
                className={cn('flex items-center gap-4 px-4 py-2.5', i !== configRows.length - 1 && 'border-b border-border-subtle')}
              >
                <span className="w-28 shrink-0 font-mono text-[11px] text-muted-foreground">{row.key}</span>
                {row.mono ? (
                  <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[12px]">{row.value}</code>
                ) : (
                  <span className="text-sm text-foreground">{row.value}</span>
                )}
              </div>
            ))}
          </div>
        </section>

        {agent.prompt_template && (
          <section>
            <h3 className="mb-2.5 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
              System Prompt
            </h3>
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-lg border border-border-subtle bg-muted p-4 font-mono text-xs leading-relaxed">
              {agent.prompt_template}
            </pre>
          </section>
        )}

        {hasConfig && (
          <section>
            <CollapsibleSection title="Config JSON" badge="set">
              <pre className="max-h-60 overflow-auto rounded-lg border border-border-subtle bg-muted p-4 font-mono text-xs">
                {configStr}
              </pre>
            </CollapsibleSection>
          </section>
        )}

        <section>
          <h3 className="mb-2.5 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
            Recent tasks
          </h3>
          {recentTasksQuery.isError ? (
            <ErrorBanner
              error={recentTasksQuery.error}
              fallback="Recent tasks failed to load"
              onRetry={recentTasksQuery.refetch}
            />
          ) : recentTasksQuery.isLoading ? (
            <div className="space-y-2 rounded-lg border border-border-subtle bg-muted p-3">
              {[0, 1, 2].map((item) => (
                <div key={item} className="flex items-center justify-between gap-3">
                  <div className="min-w-0 flex-1">
                    <Skeleton className="h-4 w-2/3" />
                    <Skeleton className="mt-2 h-3 w-1/3" />
                  </div>
                  <Skeleton className="h-5 w-16" />
                </div>
              ))}
            </div>
          ) : recentTasks.length === 0 ? (
            <div className="rounded-lg border border-dashed px-4 py-6 text-center">
              <p className="text-sm text-muted-foreground">No recent tasks for this agent</p>
            </div>
          ) : (
            <div className="overflow-hidden rounded-lg border border-border-subtle bg-muted">
              {recentTasks.map((task, index) => (
                <Link
                  key={task.id}
                  to="/tasks/$taskId"
                  params={{ taskId: task.id }}
                  className={cn(
                    'flex items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-accent/50',
                    index !== recentTasks.length - 1 && 'border-b border-border-subtle',
                  )}
                >
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium text-foreground">{task.title}</p>
                    <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                      Updated {formatDate(task.updated_at)}
                    </p>
                  </div>
                  <TaskStatusBadge status={task.status} className="shrink-0" />
                </Link>
              ))}
            </div>
          )}
        </section>

        <section>
          <h3 className="mb-2.5 font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
            Timestamps
          </h3>
          <div className="grid gap-4 sm:grid-cols-2">
            <InfoField label="Created" value={formatDate(agent.created_at)} />
            <InfoField label="Updated" value={formatDate(agent.updated_at)} />
          </div>
        </section>
      </div>
    </div>
  )
}
