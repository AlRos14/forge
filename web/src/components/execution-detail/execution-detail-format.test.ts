import { describe, expect, it } from 'vitest'

import {
  accountUsageFromLogs,
  harnessTurnCount,
  usageTotals,
} from './execution-detail-format'
import type { ExecutionUsage, LogEntry } from '@/types/generated'

function log(kind: LogEntry['kind'], payload: unknown, sequence = 1): LogEntry {
  return {
    schema_version: 1,
    sequence,
    timestamp: '2026-09-03T10:00:00Z',
    execution_id: 'exec-1',
    kind,
    stream: 'main',
    payload,
    truncated: false,
  }
}

describe('usageTotals', () => {
  it('keeps cost unknown when the harness did not report USD', () => {
    const usage: ExecutionUsage[] = [
      {
        id: 'u1',
        execution_id: 'exec-1',
        provider: 'openai',
        model: 'gpt-5',
        input_tokens: 100,
        output_tokens: 20,
        cache_read_tokens: 80,
        cache_write_tokens: 0,
        cost_usd: null,
        created_at: '2026-09-03T10:00:00Z',
      },
    ]
    expect(usageTotals(usage).costUsd).toBeNull()
  })
})

describe('harnessTurnCount', () => {
  it('counts Codex protocol turns instead of assistant log kinds', () => {
    expect(
      harnessTurnCount([
        log('session_info', {
          method: 'turn/started',
          params: { turn: { id: 'turn-1' } },
        }),
        log('tool_call', { method: 'item/started' }),
        log('session_info', {
          method: 'turn/completed',
          params: { turn: { id: 'turn-1' } },
        }),
      ]),
    ).toBe(1)
  })

  it('counts Cursor result events', () => {
    expect(harnessTurnCount([log('session_info', { type: 'result' })])).toBe(1)
  })
})

describe('accountUsageFromLogs', () => {
  it('reads the latest Codex rate-limit snapshot from loaded events', () => {
    expect(
      accountUsageFromLogs([
        log('session_info', {
          method: 'account/rateLimits/updated',
          params: { planType: 'plus', primary: { usedPercent: 1 } },
        }),
        log('session_info', {
          method: 'account/rateLimits/updated',
          params: { planType: 'plus', primary: { usedPercent: 19, windowDurationMins: 300 } },
        }),
      ]),
    ).toEqual({
      rateLimits: { planType: 'plus', primary: { usedPercent: 19, windowDurationMins: 300 } },
    })
  })
})
