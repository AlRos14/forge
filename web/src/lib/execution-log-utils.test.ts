import { describe, expect, it } from 'vitest'

import {
  compareLogsChronologically,
  logIdentity,
  mergeLogs,
  parseExecutionLogEvent,
  parseLogEntry,
} from './execution-log-utils'
import type { LogEntry } from '@/types/generated'

function makeLog(overrides: Partial<LogEntry> = {}): LogEntry {
  return {
    schema_version: 1,
    sequence: 1,
    timestamp: '2026-04-15T00:00:01Z',
    execution_id: 'exec-1',
    kind: 'assistant',
    stream: 'main',
    payload: { text: 'hello' },
    truncated: false,
    ...overrides,
  }
}

describe('execution-log-utils', () => {
  it('parses direct log objects with defaults', () => {
    const parsed = parseLogEntry({
      sequence: 2,
      timestamp: '2026-04-15T00:00:02Z',
      execution_id: 'exec-1',
      kind: 'stdout',
      payload: { line: 'ok' },
    })

    expect(parsed).toEqual({
      schema_version: 1,
      sequence: 2,
      timestamp: '2026-04-15T00:00:02Z',
      execution_id: 'exec-1',
      kind: 'stdout',
      stream: 'main',
      payload: { line: 'ok' },
      truncated: false,
    })
  })

  it('extracts execution log events from envelope payloads', () => {
    const raw = JSON.stringify({
      event_type: 'execution.log',
      execution_id: 'exec-1',
      log: {
        sequence: 9,
        timestamp: '2026-04-15T00:00:09Z',
        execution_id: 'exec-1',
        kind: 'tool_result',
        payload: { ok: true },
      },
    })

    const parsed = parseExecutionLogEvent(raw, 'exec-1')
    expect(parsed).toHaveLength(1)
    expect(parsed[0]?.sequence).toBe(9)
    expect(parsed[0]?.kind).toBe('tool_result')
  })

  it('ignores events from other executions', () => {
    const raw = JSON.stringify({
      event_type: 'execution.log',
      execution_id: 'exec-other',
      log: {
        sequence: 3,
        timestamp: '2026-04-15T00:00:03Z',
        execution_id: 'exec-other',
        kind: 'assistant',
        payload: {},
      },
    })

    expect(parseExecutionLogEvent(raw, 'exec-1')).toEqual([])
  })

  it('parses batched execution logs', () => {
    const raw = JSON.stringify({
      event_type: 'execution.log',
      execution_id: 'exec-1',
      logs: [
        {
          sequence: 1,
          timestamp: '2026-04-15T00:00:01Z',
          execution_id: 'exec-1',
          kind: 'assistant_delta',
          payload: { text: 'a' },
        },
        {
          sequence: 2,
          timestamp: '2026-04-15T00:00:02Z',
          execution_id: 'exec-1',
          kind: 'assistant',
          payload: { text: 'ab' },
        },
      ],
    })
    const parsed = parseExecutionLogEvent(raw, 'exec-1')
    expect(parsed).toHaveLength(2)
    expect(parsed[0]?.sequence).toBe(1)
    expect(parsed[1]?.sequence).toBe(2)
  })

  it('deduplicates by identity and sorts chronologically', () => {
    const logA = makeLog({ sequence: 10, timestamp: '2026-04-15T00:00:10Z' })
    const logB = makeLog({ sequence: 1, timestamp: '2026-04-15T00:00:01Z', payload: { text: 'first' } })
    const merged = mergeLogs([logA], [logB, logA])

    expect(merged).toHaveLength(2)
    expect(merged[0]?.sequence).toBe(1)
    expect(merged[1]?.sequence).toBe(10)
  })

  it('uses sequence as a tiebreaker for equal timestamps', () => {
    const earlier = makeLog({ sequence: 1, timestamp: '2026-04-15T00:00:00Z' })
    const later = makeLog({ sequence: 2, timestamp: '2026-04-15T00:00:00Z', payload: { text: 'later' } })
    expect(compareLogsChronologically(earlier, later)).toBeLessThan(0)
  })

  it('produces stable identities for equal content', () => {
    const a = makeLog({ sequence: 7 })
    const b = makeLog({ sequence: 7 })
    expect(logIdentity(a)).toBe(logIdentity(b))
  })
})
