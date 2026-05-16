import { cn } from '@/lib/cn'
import type { Execution } from '@/types/generated'

const statusConfig: Record<Execution['status'], { className: string; dot: string }> = {
  running: { className: 'bg-sky-500/15 text-sky-700 dark:text-sky-400 border-sky-500/20', dot: 'bg-sky-500' },
  completed: { className: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400 border-emerald-500/20', dot: 'bg-emerald-500' },
  failed: { className: 'bg-red-500/15 text-red-700 dark:text-red-400 border-red-500/20', dot: 'bg-red-500' },
  cancelled: { className: 'bg-zinc-500/15 text-zinc-600 dark:text-zinc-400 border-zinc-500/20', dot: 'bg-zinc-500' },
}

export function ExecutionStatusBadge({ status }: { status: Execution['status'] }) {
  const config = statusConfig[status]
  return (
    <span className={cn('inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs font-medium', config.className)}>
      {status === 'running' ? (
        <span className="relative flex h-1.5 w-1.5">
          <span className={cn('absolute inline-flex h-full w-full animate-ping rounded-full opacity-75', config.dot)} />
          <span className={cn('relative inline-flex h-1.5 w-1.5 rounded-full', config.dot)} />
        </span>
      ) : (
        <span className={cn('h-1.5 w-1.5 rounded-full', config.dot)} />
      )}
      {status}
    </span>
  )
}
