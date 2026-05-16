import type { Execution, ExecutionUsage, LogEntry } from '@/types/generated'

export function formatDate(value?: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString()
}

export function formatRelativeDate(value?: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  const now = Date.now()
  const diff = now - date.getTime()
  if (diff < 60_000) return 'just now'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`
  return date.toLocaleDateString()
}

export function executionRuntimeSeconds(execution: Execution): number {
  const started = Date.parse(execution.created_at)
  if (!Number.isFinite(started)) return 0
  const stopped =
    execution.stopped_at && Number.isFinite(Date.parse(execution.stopped_at))
      ? Date.parse(execution.stopped_at)
      : execution.status === 'running'
        ? Date.now()
        : Number.isFinite(Date.parse(execution.updated_at))
          ? Date.parse(execution.updated_at)
          : started
  return Math.max(0, Math.floor((stopped - started) / 1000))
}

export function usageTotals(usage: ExecutionUsage[]) {
  return usage.reduce(
    (totals, item) => ({
      inputTokens: totals.inputTokens + item.input_tokens,
      outputTokens: totals.outputTokens + item.output_tokens,
      cacheReadTokens: totals.cacheReadTokens + item.cache_read_tokens,
      cacheWriteTokens: totals.cacheWriteTokens + item.cache_write_tokens,
      costUsd: totals.costUsd + (item.cost_usd ?? 0),
    }),
    {
      inputTokens: 0,
      outputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      costUsd: 0,
    },
  )
}

export function latestLog(logs: LogEntry[]): LogEntry | undefined {
  return logs[logs.length - 1]
}

export function shortHash(value?: string | null): string {
  return value ? value.slice(0, 8) : '-'
}
