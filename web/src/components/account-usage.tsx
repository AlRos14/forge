import { humanize } from '@/features/federation/format'

export type UsageWindow = {
  label: string
  usedPercent: number
  resetsAt?: number | string
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value != null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function asPercent(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.min(100, Math.max(0, value))
    : null
}

function formatReset(value: number | string | undefined) {
  if (value == null) return null
  const date = typeof value === 'number' ? new Date(value * 1000) : new Date(value)
  if (Number.isNaN(date.getTime())) return `Resets ${value}`
  return `Resets ${date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  })}`
}

function windowLabel(duration: unknown, fallback: string) {
  if (duration === 300) return '5-hour limit'
  if (duration === 10_080) return 'Weekly limit'
  if (typeof duration === 'number') {
    if (duration % 1_440 === 0) return `${duration / 1_440}-day limit`
    if (duration % 60 === 0) return `${duration / 60}-hour limit`
  }
  return fallback
}

function cursorPercentFromPools(usage: Record<string, unknown>, category: string) {
  const pools = Array.isArray(usage.pools) ? usage.pools : []
  const line = pools.find((item) => typeof item === 'string' && item.startsWith(category))
  const match = typeof line === 'string' ? line.match(/(\d+(?:\.\d+)?)%/) : null
  return match ? Number(match[1]) : null
}

function isQuotaHarness(executorType?: string | null): boolean {
  return executorType === 'codex' || executorType === 'cursor'
}

export function quotaWindows(
  executorType: string | null | undefined,
  usage: Record<string, unknown> | null | undefined,
): UsageWindow[] {
  if (!usage || !isQuotaHarness(executorType)) return []

  if (executorType === 'codex') {
    const limits = asRecord(usage.rateLimits) ?? (usage.primary != null || usage.planType != null ? usage : null)
    return (['primary', 'secondary'] as const).flatMap((key, index) => {
      const window = asRecord(limits?.[key])
      const usedPercent = asPercent(window?.usedPercent)
      return usedPercent == null
        ? []
        : [
            {
              label: windowLabel(
                window?.windowDurationMins,
                index === 0 ? 'Primary limit' : 'Secondary limit',
              ),
              usedPercent,
              resetsAt: typeof window?.resetsAt === 'number' ? window.resetsAt : undefined,
            },
          ]
    })
  }

  const categories = asRecord(usage.categories)
  return (
    [
      ['Included', 'included'],
      ['Auto', 'auto'],
      ['API', 'api'],
    ] as const
  ).flatMap(([label, key]) => {
    const usedPercent = asPercent(categories?.[key]) ?? cursorPercentFromPools(usage, label)
    return usedPercent == null ? [] : [{ label, usedPercent }]
  })
}

export function compactQuotaLabel(windows: UsageWindow[]): string | null {
  if (windows.length === 0) return null
  const busiest = windows.reduce((max, window) =>
    window.usedPercent > max.usedPercent ? window : max,
  )
  return `${Math.round(busiest.usedPercent)}% used`
}

function UsageMeter({ window }: { window: UsageWindow }) {
  const remaining = Math.max(0, 100 - window.usedPercent)
  return (
    <div className="rounded-md border border-border-subtle bg-card px-3 py-3">
      <div className="flex items-baseline justify-between gap-3 text-xs">
        <span className="font-medium text-foreground">{window.label}</span>
        <span className="font-mono tabular-nums text-muted-foreground">{remaining}% left</span>
      </div>
      <div
        className="mt-2 h-2 overflow-hidden rounded-full bg-muted"
        role="progressbar"
        aria-label={`${window.label} used`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={window.usedPercent}
      >
        <div className="h-full rounded-full bg-primary" style={{ width: `${window.usedPercent}%` }} />
      </div>
      <div className="mt-1.5 flex justify-between gap-3 font-mono text-micro text-muted-foreground">
        <span>{window.usedPercent}% used</span>
        {formatReset(window.resetsAt) ? <span>{formatReset(window.resetsAt)}</span> : null}
      </div>
    </div>
  )
}

export function AccountUsageDetails({
  executorType,
  usage,
  className = 'mt-3 space-y-2.5',
}: {
  executorType: string
  usage: Record<string, unknown>
  className?: string
}) {
  const windows = quotaWindows(executorType, usage)

  if (executorType === 'codex') {
    const limits = asRecord(usage.rateLimits) ?? usage
    const plan = typeof limits.planType === 'string' ? humanize(limits.planType) : 'Codex'
    return (
      <div className={className}>
        <p className="text-xs text-muted-foreground">
          <span className="font-medium text-foreground">{plan} plan</span> · Codex account
        </p>
        {windows.map((window) => (
          <UsageMeter key={window.label} window={window} />
        ))}
      </div>
    )
  }

  if (executorType === 'cursor') {
    const plan = typeof usage.plan === 'string' ? usage.plan : 'Cursor'
    const reset = typeof usage.resets_at === 'string' ? usage.resets_at : undefined
    return (
      <div className={className}>
        <p className="text-xs text-muted-foreground">
          <span className="font-medium text-foreground">{plan} plan</span>
          {reset ? ` · Resets ${reset}` : ''}
          {usage.on_demand_enabled === false ? ' · On-demand disabled' : ''}
        </p>
        {windows.map((window) => (
          <UsageMeter key={window.label} window={window} />
        ))}
      </div>
    )
  }

  return (
    <p className="mt-3 text-xs text-muted-foreground">
      Usage was refreshed, but this provider does not expose a visual summary.
    </p>
  )
}
