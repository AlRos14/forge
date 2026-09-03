import { Info } from '@phosphor-icons/react'

import { AccountUsageDetails, compactQuotaLabel, quotaWindows } from '@/components/account-usage'
import {
  formatCostUsd,
  formatRuntimeSeconds,
  formatTokenCount,
} from '@/components/task-execution-observability'
import { Tooltip } from '@/components/ui/tooltip'
import {
  accountUsageFromLogs,
  asUsageRecord,
  executionRuntimeSeconds,
  formatDate,
  formatRelativeDate,
  harnessTurnCount,
  latestLog,
  usageTotals,
} from '@/components/execution-detail/execution-detail-format'
import type { Execution, ExecutionUsage, LogEntry } from '@/types/generated'

function Metric({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="rounded-md border bg-background px-3 py-2" title={title}>
      <p className="font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
        {label}
      </p>
      <p className="mt-1 truncate font-mono text-sm font-semibold tabular-nums text-foreground">
        {value}
      </p>
    </div>
  )
}

export function ExecutionObservabilitySection({
  execution,
  logs,
  usage,
  executorType,
  accountUsage,
}: {
  execution: Execution
  logs: LogEntry[]
  usage: ExecutionUsage[]
  executorType?: string | null
  accountUsage?: Record<string, unknown> | null
}) {
  const totals = usageTotals(usage)
  const totalTokens =
    totals.inputTokens +
    totals.outputTokens +
    totals.cacheReadTokens +
    totals.cacheWriteTokens
  const turns = harnessTurnCount(logs)
  const recentLog = latestLog(logs)
  const resolvedUsage =
    accountUsage ?? accountUsageFromLogs(logs) ?? asUsageRecord(execution.account_usage)
  const windows = quotaWindows(executorType, resolvedUsage)
  const quotaLabel = compactQuotaLabel(windows)
  const showCost = totals.costUsd != null
  const showQuotaTile = !showCost && quotaLabel != null

  return (
    <section className="space-y-3">
      <div className="flex items-center gap-1.5 text-micro font-medium uppercase tracking-wider text-muted-foreground">
        <Info className="h-3 w-3" />
        <span>Observability</span>
      </div>
      <div className="grid grid-cols-2 gap-2">
        <Metric label="Runtime" value={formatRuntimeSeconds(executionRuntimeSeconds(execution))} />
        <Metric label="Turns" value={turns.toLocaleString()} />
        <Metric
          label="Tokens"
          title={`${formatTokenCount(totals.inputTokens)} input / ${formatTokenCount(totals.outputTokens)} output / ${formatTokenCount(totals.cacheReadTokens)} cache read / ${formatTokenCount(totals.cacheWriteTokens)} cache write`}
          value={formatTokenCount(totalTokens, true)}
        />
        {showQuotaTile ? (
          <Metric label="Quota" value={quotaLabel} title="Account quota after this run" />
        ) : (
          <Metric
            label="Cost"
            value={formatCostUsd(totals.costUsd)}
            title={
              totals.costUsd == null
                ? 'USD cost is only reported for on-demand API billing'
                : undefined
            }
          />
        )}
      </div>
      {resolvedUsage && windows.length > 0 && executorType ? (
        <AccountUsageDetails
          className="space-y-2.5"
          executorType={executorType}
          usage={resolvedUsage}
        />
      ) : null}
      <div className="rounded-md border bg-background px-3 py-2">
        <div className="flex items-center justify-between gap-3">
          <p className="font-mono text-micro font-semibold uppercase tracking-[0.8px] text-muted-foreground">
            Last Event
          </p>
          {recentLog ? (
            <Tooltip content={formatDate(recentLog.timestamp)}>
              <span className="shrink-0 text-micro text-muted-foreground">
                {formatRelativeDate(recentLog.timestamp)}
              </span>
            </Tooltip>
          ) : null}
        </div>
        <p className="mt-1 truncate text-sm text-foreground">
          {recentLog ? recentLog.kind.replace(/_/g, ' ') : 'No loaded events'}
        </p>
        <p className="mt-0.5 text-xs text-muted-foreground">
          {logs.length.toLocaleString()} loaded log events
        </p>
      </div>
    </section>
  )
}
