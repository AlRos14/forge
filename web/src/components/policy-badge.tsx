import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/cn'
import type { EffectiveExecutionPolicy } from '@/types/generated'

function policyLabel(value: string): string {
  return value
    .replace(/_/g, ' ')
    .replace(/\b\w/g, (c) => c.toUpperCase())
}

export function PolicyBadge({ policy, className }: { policy: EffectiveExecutionPolicy; className?: string }) {
  return (
    <div className={cn('flex min-w-0 flex-wrap items-center gap-1.5', className)}>
      <Badge variant="outline" className="max-w-full truncate text-micro uppercase">
        {policyLabel(policy.executor_kind)}
      </Badge>
      <Badge variant="secondary" className="max-w-full truncate text-micro">
        {policyLabel(policy.isolation_posture)}
      </Badge>
      {policy.is_high_risk ? (
        <Badge className="border border-red-500/25 bg-red-500/15 text-micro uppercase text-red-700 dark:text-red-400">
          High Risk
        </Badge>
      ) : null}
    </div>
  )
}
