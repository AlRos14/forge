import { Avatar } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/cn'
import type { Agent } from '@/types/generated'
import { executorDisplayNames } from './constants'

export function AgentList({
  agents,
  selectedId,
  onSelect,
}: {
  agents: Agent[]
  selectedId?: string
  onSelect: (id: string) => void
}) {
  return (
    <div className="p-1.5">
      {agents.map((agent) => {
        const isSelected = selectedId === agent.id
        const isOnline = agent.status === 'idle' || agent.status === 'busy'
        return (
          <button
            key={agent.id}
            type="button"
            className={cn(
              'flex w-full cursor-pointer items-center gap-2 rounded-lg px-3 py-2.5 text-left transition-colors',
              isSelected
                ? 'border border-primary/20 bg-[var(--ember-surface)] text-foreground'
                : 'border border-transparent text-muted-foreground hover:bg-accent/50 hover:text-foreground',
            )}
            onClick={() => onSelect(agent.id)}
          >
            <Avatar name={agent.name} seed={agent.id} size="sm" />
            <div className="min-w-0 flex-1">
              <p className="truncate text-ui font-medium">{agent.name}</p>
              <div className="mt-0.5 flex min-w-0 items-center gap-1.5">
                <p className="truncate font-mono text-micro text-muted-foreground">
                  {executorDisplayNames[agent.executor_type] ?? agent.executor_type}
                  {' · '}
                  {agent.active_task_count ?? 0} tasks
                </p>
                {agent.paused ? (
                  <Badge variant="outline" className="shrink-0 text-micro uppercase">
                    Paused
                  </Badge>
                ) : null}
              </div>
              {agent.model ? (
                <p className="truncate font-mono text-micro text-muted-foreground">
                  {agent.model}
                  {agent.reasoning_effort ? ` · ${agent.reasoning_effort}` : ''}
                </p>
              ) : null}
            </div>
            <span
              className={cn(
                'h-1.5 w-1.5 shrink-0 rounded-full',
                isOnline ? 'bg-primary' : 'bg-muted-foreground',
                isOnline && isSelected && 'shadow-[0_0_6px_rgba(249,115,22,0.5)]',
              )}
            />
          </button>
        )
      })}
    </div>
  )
}
