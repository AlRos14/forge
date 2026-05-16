import { ArrowRight, ArrowSquareOut, Info } from '@phosphor-icons/react'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/cn'
import type { Task } from '@/types/generated'

function badgeClass(value: string): string {
  const normalized = value.toLowerCase()
  if (normalized === 'merged' || normalized === 'clean' || normalized === 'mergeable') {
    return 'bg-green-100 text-green-900 dark:bg-green-950 dark:text-green-300'
  }
  if (normalized === 'closed' || normalized === 'blocked' || normalized === 'conflicting') {
    return 'bg-red-100 text-red-900 dark:bg-red-950 dark:text-red-300'
  }
  return 'bg-blue-100 text-blue-900 dark:bg-blue-950 dark:text-blue-300'
}

export function TaskPrSummaryCard({ task }: { task: Task }) {
  const summary = task.pr_summary
  if (!summary) return null

  return (
    <div className="space-y-3 rounded-lg border border-border-subtle bg-card p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <p className="font-medium">Pull Request</p>
          <div className="mt-1 flex flex-wrap items-center gap-2">
            <Badge className={cn('border-transparent', badgeClass(summary.pr_state))}>
              {summary.pr_state}
            </Badge>
            <Badge className={cn('border-transparent', badgeClass(summary.merge_status))}>
              {summary.merge_status}
            </Badge>
          </div>
        </div>
        {summary.pr_url ? (
          <a
            className="inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-sm font-medium hover:bg-accent"
            href={summary.pr_url}
            rel="noreferrer"
            target="_blank"
          >
            Open PR
            <ArrowSquareOut className="h-3.5 w-3.5" />
          </a>
        ) : null}
      </div>

      {task.awaiting_human ? (
        <div className="flex items-start gap-2 rounded-md border border-blue-300 bg-blue-50 p-3 text-sm text-blue-950 dark:border-blue-800 dark:bg-blue-950 dark:text-blue-100">
          <Info className="mt-0.5 h-4 w-4 shrink-0" />
          <span>Awaiting human merge</span>
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="rounded-md bg-muted px-2 py-1 font-mono text-xs">
          {summary.source_branch}
        </span>
        <ArrowRight className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="rounded-md bg-muted px-2 py-1 font-mono text-xs">
          {summary.target_branch}
        </span>
      </div>
    </div>
  )
}
