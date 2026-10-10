import { describe, expect, it, vi } from 'vitest'
import type { QueryClient } from '@tanstack/react-query'
import type { ForgeEvent, PublicEventType } from '@/types/generated'
import { routeSsePayload } from './sse'

function event(
  event_type: string,
  fields: Partial<ForgeEvent> = {},
): ForgeEvent {
  return {
    event_type: event_type as PublicEventType,
    entity_id: 'entity-1',
    timestamp: '2026-10-10T00:00:00Z',
    event_id: null,
    sequence: null,
    entity_type: null,
    scope_type: null,
    scope_id: null,
    payload: null,
    ...fields,
  }
}

function createMocks() {
  const invalidateQueries = vi.fn()
  const queryClient = { invalidateQueries } as unknown as QueryClient
  const dispatch = vi.fn()
  return { queryClient, invalidateQueries, dispatch }
}

describe('routeSsePayload', () => {
  it('invalidates Task and Gate projections for a durable lifecycle event', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      event('task.lifecycle_changed', {
        entity_id: 'task-1',
        entity_type: 'task',
        scope_type: 'task',
        scope_id: 'task-1',
        payload: { task_id: 'task-1', to_state: 'active' },
      }),
      queryClient,
      { dispatch },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['tasks', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'detail'] }],
        [{ queryKey: ['tasks', 'task-1', 'gates'] }],
      ]),
    )
    expect(dispatch).not.toHaveBeenCalled()
  })

  it('invalidates exact Task-scoped execution projections', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('execution.completed', {
        entity_id: 'execution-1',
        entity_type: 'execution',
        scope_type: 'task',
        scope_id: 'task-1',
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['executions', 'execution-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'executions'] }],
        [{ queryKey: ['tasks', 'task-1', 'diff'] }],
      ]),
    )
  })

  it('keeps retired status, Agent Chat, and legacy Review events out of UI projections', () => {
    for (const event_type of ['task.status_changed', 'agent_chat.message_created', 'review.passed']) {
      const { queryClient, invalidateQueries, dispatch } = createMocks()
      routeSsePayload(
        event(event_type, { entity_id: 'legacy-1' }),
        queryClient,
        { dispatch },
      )
      expect(invalidateQueries).not.toHaveBeenCalled()
      expect(dispatch).not.toHaveBeenCalled()
    }
  })

  it('resynchronizes active queries when the public stream reports lag', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('events.resync_required', {
        entity_id: 'events.resync_required',
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries).toHaveBeenCalledWith(
      expect.objectContaining({ refetchType: 'active' }),
    )
  })
})
