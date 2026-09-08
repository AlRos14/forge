import { cn } from '@/lib/cn'

export function taskStateConfigWithPlanReview(
  enabled: boolean,
): Record<string, unknown> | undefined {
  return enabled ? { plan_review: true } : undefined
}

export function PlanReviewCheckbox({
  checked,
  onChange,
  disabled,
  compact = false,
}: {
  checked: boolean
  onChange: (enabled: boolean) => void
  disabled?: boolean
  compact?: boolean
}) {
  return (
    <label className={cn('flex items-start gap-2', compact ? 'text-xs' : 'text-sm')}>
      <input
        type="checkbox"
        className="mt-0.5"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>
        <span className="font-medium">Review plan before coding</span>
        {compact ? (
          <span className="mt-0.5 block text-[11px] leading-snug text-muted-foreground">
            Optional. Reviewer comments, then you approve.
          </span>
        ) : (
          <span className="mt-0.5 block text-xs text-muted-foreground">
            Off by default. Planner writes a revision, the Reviewer (new session) comments, the
            planner revises, and review runs again. Reviewer pass waits for you before coding.
            Findings go back to the planner, not the coder.
          </span>
        )}
      </span>
    </label>
  )
}
