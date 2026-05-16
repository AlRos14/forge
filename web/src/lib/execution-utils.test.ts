import { describe, expect, it } from 'vitest'

import { buildExecutionChains, isResumeExecution, turnLabel } from '@/lib/execution-utils'
import type { Execution } from '@/types/generated'

function execution(overrides: Partial<Execution>): Execution {
  return {
    id: overrides.id ?? 'execution',
    task_id: 'task',
    role: overrides.role ?? 'coder',
    status: overrides.status ?? 'completed',
    created_at: overrides.created_at ?? '2026-05-02T10:00:00.000Z',
    updated_at: overrides.updated_at ?? '2026-05-02T10:00:00.000Z',
    ...overrides,
  }
}

describe('execution utils', () => {
  it('detects resume executions from dispatch metadata and executor fallback config', () => {
    expect(
      isResumeExecution(
        execution({
          executor_config_snapshot: {
            dispatch: { execution_policy: 'resume_latest_target_role_thread' },
          },
        }),
      ),
    ).toBe(true)

    expect(
      isResumeExecution(
        execution({
          executor_config_snapshot: {
            config: { resume_thread_in_place: true },
          },
        }),
      ),
    ).toBe(true)

    expect(
      isResumeExecution(
        execution({
          executor_config_snapshot: {
            dispatch: { execution_policy: 'new_execution' },
          },
        }),
      ),
    ).toBe(false)
  })

  it('groups follow-up turns under their root and sorts chains by active/latest turn', () => {
    const oldRoot = execution({
      id: 'old-root',
      created_at: '2026-05-02T10:00:00.000Z',
    })
    const child = execution({
      id: 'child',
      parent_execution_id: oldRoot.id,
      status: 'running',
      created_at: '2026-05-02T10:05:00.000Z',
      executor_config_snapshot: {
        dispatch: { execution_policy: 'resume_latest_target_role_thread' },
      },
    })
    const newerRoot = execution({
      id: 'newer-root',
      created_at: '2026-05-02T10:03:00.000Z',
    })

    const chains = buildExecutionChains([newerRoot, child, oldRoot])

    expect(chains.map((chain) => chain.root.id)).toEqual(['old-root', 'newer-root'])
    expect(chains[0].turns.map((turn) => turn.id)).toEqual(['old-root', 'child'])
    expect(turnLabel(0, oldRoot)).toBe('Initial run')
    expect(turnLabel(1, child)).toBe('Follow-up turn')
  })
})
