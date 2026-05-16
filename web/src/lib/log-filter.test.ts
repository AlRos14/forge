import { describe, expect, it } from 'vitest'

import { effectiveLogFilterKind } from './log-filter'
import type { LogEntry } from '@/types/generated'

function log(kind: LogEntry['kind'], payload: unknown): LogEntry {
  return {
    schema_version: 1,
    sequence: 1,
    timestamp: '2026-04-15T00:00:01Z',
    execution_id: 'exec-1',
    kind,
    stream: 'main',
    payload,
    truncated: false,
  }
}

describe('effectiveLogFilterKind', () => {
  it('maps assistant deltas into the assistant filter', () => {
    expect(effectiveLogFilterKind(log('assistant_delta', { delta: 'hi' }))).toBe('assistant')
  })

  it('maps Codex session item events to their user-facing filter categories', () => {
    expect(
      effectiveLogFilterKind(
        log('session_info', {
          method: 'item/completed',
          params: { item: { type: 'userMessage' } },
        }),
      ),
    ).toBe('user')

    expect(
      effectiveLogFilterKind(
        log('session_info', {
          method: 'item/completed',
          params: { item: { type: 'fileChange' } },
        }),
      ),
    ).toBe('file_change')

    expect(
      effectiveLogFilterKind(
        log('session_info', {
          method: 'item/started',
          params: { item: { type: 'commandExecution' } },
        }),
      ),
    ).toBe('shell_command')

    expect(
      effectiveLogFilterKind(
        log('session_info', {
          method: 'item/commandExecution/outputDelta',
          params: { itemId: 'cmd-1', delta: 'ok\n' },
        }),
      ),
    ).toBe('stdout')
  })

  it('keeps true session lifecycle events under the session filter', () => {
    expect(
      effectiveLogFilterKind(
        log('session_info', {
          method: 'thread/started',
          result: { thread: { id: 'thread-1' } },
        }),
      ),
    ).toBe('session_info')
  })
})
