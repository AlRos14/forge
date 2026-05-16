import type { LogEntry } from '@/types/generated'

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isLogKind(value: unknown): value is LogEntry['kind'] {
  return (
    value === 'stdout' ||
    value === 'stderr' ||
    value === 'tool_call' ||
    value === 'tool_result' ||
    value === 'assistant' ||
    value === 'assistant_delta' ||
    value === 'user' ||
    value === 'system' ||
    value === 'file_change' ||
    value === 'shell_command' ||
    value === 'approval_question' ||
    value === 'session_info' ||
    value === 'unknown'
  )
}

export function compareLogsChronologically(a: LogEntry, b: LogEntry): number {
  const timestampOrder = Date.parse(a.timestamp) - Date.parse(b.timestamp)
  if (Number.isFinite(timestampOrder) && timestampOrder !== 0) return timestampOrder
  return a.sequence - b.sequence
}

export function logIdentity(log: LogEntry): string {
  return `${log.execution_id}:${log.sequence}:${log.timestamp}:${log.kind}:${JSON.stringify(log.payload)}`
}

export function mergeLogs(existing: LogEntry[], incoming: LogEntry[]): LogEntry[] {
  const seen = new Set<string>()
  const merged: LogEntry[] = []
  for (const log of [...existing, ...incoming]) {
    const key = logIdentity(log)
    if (seen.has(key)) continue
    seen.add(key)
    merged.push(log)
  }
  return merged.sort(compareLogsChronologically)
}

export function parseLogEntry(value: unknown): LogEntry | undefined {
  if (!isRecord(value)) return undefined
  if (typeof value.sequence !== 'number') return undefined
  if (!isLogKind(value.kind)) return undefined
  if (typeof value.timestamp !== 'string') return undefined
  return {
    schema_version: typeof value.schema_version === 'number' ? value.schema_version : 1,
    sequence: value.sequence,
    timestamp: value.timestamp,
    execution_id: typeof value.execution_id === 'string' ? value.execution_id : '',
    kind: value.kind,
    stream: value.stream === 'heartbeat' ? 'heartbeat' : 'main',
    payload: value.payload,
    truncated: typeof value.truncated === 'boolean' ? value.truncated : false,
  }
}

export function parseExecutionLogEvent(raw: string, executionId: string): LogEntry[] {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return []
  }
  if (!isRecord(parsed)) return []
  if (parsed.event_type && parsed.event_type !== 'execution.log') return []
  if (parsed.execution_id !== executionId && parsed.entity_id !== executionId) return []

  const logs = Array.isArray(parsed.logs)
    ? parsed.logs.map(parseLogEntry).filter((log): log is LogEntry => Boolean(log))
    : []
  if (logs.length > 0) return logs

  const single = parseLogEntry(parsed.log) ?? parseLogEntry(parsed)
  return single ? [single] : []
}
