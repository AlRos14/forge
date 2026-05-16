import { useState } from 'react'
import { CaretDown, WarningCircle } from '@phosphor-icons/react'
import { Checkbox } from '@/components/ui/checkbox'
import { cn } from '@/lib/cn'
import type { PlanArtifactDetail, PlanChecklistItem, PlanProgressSummary } from '@/types/generated'

function percent(progress: PlanProgressSummary): number {
  if (progress.total <= 0) return 0
  return Math.min(100, Math.max(0, Math.round((progress.completed / progress.total) * 100)))
}

function ChecklistItem({ item }: { item: PlanChecklistItem }) {
  return (
    <div
      className="flex min-w-0 items-start gap-2 rounded py-1.5 text-sm transition-colors hover:bg-muted/40"
      style={{ paddingLeft: `${Math.min(item.nesting_level, 6) * 14}px` }}
    >
      <Checkbox checked={item.checked} readOnly disabled className="mt-0.5 shrink-0" />
      <span
        className={cn(
          'min-w-0 flex-1 break-words leading-relaxed',
          item.checked && 'text-muted-foreground line-through',
        )}
      >
        {item.label}
      </span>
    </div>
  )
}

export function PlanChecklist({
  progress,
  artifact,
  className,
}: {
  progress?: PlanProgressSummary | null
  artifact?: PlanArtifactDetail | null
  className?: string
}) {
  const [expanded, setExpanded] = useState(false)

  if (!progress || !progress.available) {
    return (
      <div className={cn('rounded-lg border border-dashed bg-muted/20 p-3 text-sm text-muted-foreground', className)}>
        No plan artifact available
      </div>
    )
  }

  const warnings = [...progress.warnings, ...(artifact?.warnings ?? [])]
  const items = artifact?.items ?? []
  const pct = percent(progress)

  return (
    <div className={cn('rounded-lg border bg-muted/20', className)}>
      <button
        type="button"
        className="flex w-full cursor-pointer items-center gap-3 p-3 text-left transition-colors hover:bg-muted/30"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        <div className="min-w-0 flex-1 space-y-1.5">
          <div className="h-1.5 overflow-hidden rounded-full bg-muted">
            <div
              className="h-full rounded-full bg-primary transition-all duration-500 ease-out"
              style={{ width: `${pct}%` }}
            />
          </div>
          <span className="font-mono text-[11px] text-muted-foreground">
            {progress.completed}/{progress.total} completed
          </span>
        </div>
        <CaretDown
          className={cn('h-3.5 w-3.5 shrink-0 text-muted-foreground/50 transition-transform duration-200', expanded && 'rotate-180')}
        />
      </button>

      {expanded && (
        <div className="border-t px-3 pb-3 pt-2 space-y-2">
          {warnings.length > 0 ? (
            <div className="space-y-1">
              {warnings.map((warning, index) => (
                <div
                  key={`${warning}-${index}`}
                  className="flex items-start gap-2 rounded-md border border-amber-500/20 bg-amber-500/10 px-2.5 py-1.5 text-xs text-amber-700 dark:text-amber-300"
                >
                  <WarningCircle size={14} className="mt-0.5 shrink-0" />
                  <span className="min-w-0 break-words">{warning}</span>
                </div>
              ))}
            </div>
          ) : null}

          {items.length > 0 ? (
            <div className="divide-y divide-border-subtle">
              {items.map((item) => (
                <ChecklistItem key={`${item.line_number}-${item.label}`} item={item} />
              ))}
            </div>
          ) : null}
        </div>
      )}
    </div>
  )
}
