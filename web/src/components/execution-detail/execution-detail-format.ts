import { effectiveLogFilterKind } from '@/lib/log-filter'
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
      costUsd:
        item.cost_usd == null
          ? totals.costUsd
          : (totals.costUsd ?? 0) + item.cost_usd,
    }),
    {
      inputTokens: 0,
      outputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      costUsd: null as number | null,
    },
  )
}

export function latestLog(logs: LogEntry[]): LogEntry | undefined {
  return logs[logs.length - 1]
}

export function shortHash(value?: string | null): string {
  return value ? value.slice(0, 8) : '-'
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined
}

function stringField(record: Record<string, unknown> | undefined, field: string): string | undefined {
  const value = record?.[field]
  return typeof value === 'string' ? value : undefined
}

function turnIdFromPayload(payload: unknown): string | undefined {
  const record = asRecord(payload)
  const params = asRecord(record?.params)
  const turn = asRecord(params?.turn) ?? asRecord(record?.turn)
  return (
    stringField(turn, 'id') ??
    stringField(params, 'turnId') ??
    stringField(params, 'turn_id') ??
    stringField(record, 'turnId') ??
    stringField(record, 'turn_id')
  )
}

function payloadMethod(payload: unknown): string {
  return stringField(asRecord(payload), 'method')?.toLowerCase() ?? ''
}

function payloadType(payload: unknown): string {
  return stringField(asRecord(payload), 'type')?.toLowerCase() ?? ''
}

/** Count harness model turns, not raw assistant log lines in the loaded window. */
export function harnessTurnCount(logs: LogEntry[]): number {
  const turnIds = new Set<string>()
  let completedTurns = 0
  let cursorResults = 0
  let assistantMessages = 0

  for (const log of logs) {
    const method = payloadMethod(log.payload)
    const type = payloadType(log.payload)
    const turnId = turnIdFromPayload(log.payload)

    if (method === 'turn/started' || method === 'turn/completed') {
      if (turnId) turnIds.add(turnId)
      else if (method === 'turn/completed') completedTurns += 1
    }

    if (type === 'result' || (log.kind === 'session_info' && type === 'result')) {
      cursorResults += 1
    }

    if (effectiveLogFilterKind(log) === 'assistant') {
      assistantMessages += 1
    }
  }

  if (turnIds.size > 0) return turnIds.size
  if (completedTurns > 0) return completedTurns
  if (cursorResults > 0) return cursorResults
  return assistantMessages
}

export function accountUsageFromLogs(logs: LogEntry[]): Record<string, unknown> | null {
  for (let index = logs.length - 1; index >= 0; index -= 1) {
    const payload = asRecord(logs[index]?.payload)
    if (stringField(payload, 'method') !== 'account/rateLimits/updated') continue
    const params = asRecord(payload?.params) ?? payload
    if (!params) continue
    return params.rateLimits != null ? params : { rateLimits: params }
  }
  return null
}

export function asUsageRecord(value: unknown): Record<string, unknown> | null {
  return asRecord(value) ?? null
}
