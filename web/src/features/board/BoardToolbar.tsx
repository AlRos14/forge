import type { RefObject } from 'react'
import { Funnel, MagnifyingGlass, Plus, X } from '@phosphor-icons/react'
import { AgentFilterGroup } from '@/components/agent-filter-group'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/cn'
import type { Agent } from '@/types/generated'

export type BoardFilterPatch = {
  agentIds?: string[]
  priorityMax?: number
  priorityMin?: number
  q?: string
  blockedOnly?: boolean
  includeCancelled?: boolean
  includeArchived?: boolean
}

export function BoardToolbar({
  agents,
  selectedAgentIds,
  q,
  priorityMin,
  priorityMax,
  blockedOnly,
  includeCancelled,
  includeArchived,
  showMobileFilters,
  searchInputRef,
  orderingMessage,
  onToggleMobileFilters,
  onFilterChange,
  onNewTask,
}: {
  agents: Agent[]
  selectedAgentIds: string[]
  q: string
  priorityMin?: number
  priorityMax?: number
  blockedOnly: boolean
  includeCancelled: boolean
  includeArchived: boolean
  showMobileFilters: boolean
  searchInputRef: RefObject<HTMLInputElement>
  orderingMessage?: string
  onToggleMobileFilters: () => void
  onFilterChange: (patch: BoardFilterPatch) => void
  onNewTask: () => void
}) {
  const hasActiveFilters =
    selectedAgentIds.length > 0 ||
    priorityMin !== undefined ||
    priorityMax !== undefined ||
    blockedOnly ||
    includeCancelled ||
    includeArchived
  const activeFilterCount = [
    selectedAgentIds.length > 0,
    priorityMin !== undefined || priorityMax !== undefined,
    blockedOnly,
    includeCancelled,
    includeArchived,
  ].filter(Boolean).length

  return (
    <div className="shrink-0 space-y-2" data-board-toolbar>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <div className="relative">
            <MagnifyingGlass
              size={15}
              className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground"
            />
            <input
              ref={searchInputRef}
              aria-label="Search board tasks"
              className="h-8 w-52 rounded-lg border border-input bg-background pl-8 pr-3 text-sm placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-1 focus:ring-offset-background"
              placeholder="Search tasks..."
              value={q}
              onChange={(event) => onFilterChange({ q: event.target.value })}
            />
          </div>

          <button
            type="button"
            className={cn(
              'flex h-8 cursor-pointer items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors hover:bg-accent md:hidden',
              hasActiveFilters || showMobileFilters
                ? 'border-foreground/20 bg-foreground/5 text-foreground'
                : 'text-muted-foreground',
            )}
            onClick={onToggleMobileFilters}
          >
            <Funnel size={14} />
            Filters
            {activeFilterCount > 0 ? (
              <span className="flex h-4 w-4 items-center justify-center rounded-full bg-foreground text-micro text-background">
                {activeFilterCount}
              </span>
            ) : null}
          </button>

          <div className="hidden h-4 w-px bg-border md:block" />
          <div
            className={cn(
              'flex flex-wrap items-center gap-2',
              showMobileFilters ? 'w-full md:w-auto' : 'hidden md:flex',
            )}
          >
            {agents.length > 0 ? (
              <>
                <AgentFilterGroup
                  agents={agents}
                  selectedAgentIds={selectedAgentIds}
                  onSelect={(agentIds) => onFilterChange({ agentIds })}
                />
                <div className="h-4 w-px bg-border" />
              </>
            ) : null}
            {[
              { key: 'blockedOnly' as const, label: 'Blocked', active: blockedOnly },
              { key: 'includeCancelled' as const, label: 'Cancelled', active: includeCancelled },
              { key: 'includeArchived' as const, label: 'Archived', active: includeArchived },
            ].map(({ key, label, active }) => (
              <button
                key={key}
                type="button"
                className={cn(
                  'flex h-7 cursor-pointer items-center rounded-full px-2.5 text-xs font-medium transition-colors',
                  active
                    ? 'bg-foreground/10 text-foreground ring-1 ring-inset ring-foreground/20'
                    : 'text-muted-foreground hover:bg-accent hover:text-foreground',
                )}
                onClick={() => onFilterChange({ [key]: !active })}
              >
                {label}
              </button>
            ))}
            <div className="h-4 w-px bg-border" />
            <div className="flex items-center gap-1.5">
              <span className="text-xs font-medium text-muted-foreground">Priority</span>
              <input
                aria-label="Minimum priority"
                className="h-7 w-16 rounded-lg border bg-background px-2 text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                min={0}
                placeholder="Min"
                type="number"
                value={priorityMin ?? ''}
                onChange={(event) =>
                  onFilterChange({
                    priorityMin: event.target.value === '' ? undefined : Number(event.target.value),
                  })
                }
              />
              <span className="text-xs text-muted-foreground">–</span>
              <input
                aria-label="Maximum priority"
                className="h-7 w-16 rounded-lg border bg-background px-2 text-xs focus:outline-none focus:ring-2 focus:ring-ring"
                min={0}
                placeholder="Max"
                type="number"
                value={priorityMax ?? ''}
                onChange={(event) =>
                  onFilterChange({
                    priorityMax: event.target.value === '' ? undefined : Number(event.target.value),
                  })
                }
              />
            </div>
            {priorityMin !== undefined || priorityMax !== undefined ? (
              <span className="flex items-center gap-1 rounded-full border border-border bg-muted/50 py-1 pl-2.5 pr-1.5 text-xs text-foreground">
                Priority: {priorityMin ?? '0'}–{priorityMax ?? '∞'}
                <button
                  type="button"
                  aria-label="Clear priority filter"
                  className="cursor-pointer text-muted-foreground transition-colors hover:text-foreground"
                  onClick={() => onFilterChange({ priorityMin: undefined, priorityMax: undefined })}
                >
                  <X size={10} weight="bold" />
                </button>
              </span>
            ) : null}
            {hasActiveFilters || q ? (
              <button
                type="button"
                className="cursor-pointer text-xs text-muted-foreground transition-colors hover:text-foreground"
                onClick={() =>
                  onFilterChange({
                    agentIds: [],
                    priorityMin: undefined,
                    priorityMax: undefined,
                    blockedOnly: false,
                    includeCancelled: false,
                    includeArchived: false,
                    q: '',
                  })
                }
              >
                Clear all
              </button>
            ) : null}
          </div>
        </div>

        <Button size="sm" className="h-8 gap-1.5 rounded-lg text-xs" onClick={onNewTask}>
          <Plus size={14} weight="bold" />
          New Task
        </Button>
      </div>
      {orderingMessage ? (
        <p className="text-xs text-muted-foreground" role="status" data-ordering-status>
          {orderingMessage}
        </p>
      ) : null}
    </div>
  )
}
