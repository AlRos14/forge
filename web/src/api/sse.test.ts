import { describe, expect, it, vi } from 'vitest'
import type { QueryClient } from '@tanstack/react-query'
import { routeSsePayload } from './sse'

function createMocks() {
  const invalidateQueries = vi.fn()
  const queryClient = { invalidateQueries } as unknown as QueryClient
  const dispatch = vi.fn()
  return { queryClient, invalidateQueries, dispatch }
}

describe('routeSsePayload', () => {
  it('does not invalidate broad queries for execution.log', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'execution.log',
        entity_id: 'exec-1',
        task_id: 'task-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(dispatch).not.toHaveBeenCalled()
  })

  it('invalidates agent usage when an execution log reports rate limits', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'execution.log',
        entity_id: 'exec-1',
        task_id: 'task-1',
        timestamp: '2026-05-05T00:00:00Z',
        logs: [
          {
            kind: 'session_info',
            payload: {
              method: 'account/rateLimits/updated',
              params: { planType: 'plus', primary: { usedPercent: 21 } },
            },
          },
        ],
      },
      queryClient,
      { dispatch },
    )
    expect(dispatch).not.toHaveBeenCalled()
    expect(invalidateQueries).toHaveBeenCalledTimes(1)
    const predicate = invalidateQueries.mock.calls[0]?.[0]?.predicate as
      | ((query: { queryKey: unknown[] }) => boolean)
      | undefined
    expect(predicate?.({ queryKey: ['federated-agents', 'agent-1', 'usage'] })).toBe(true)
    expect(predicate?.({ queryKey: ['federated-agents'] })).toBe(false)
  })

  it('invalidates execution/task/agents for execution terminal and start events', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'execution.started',
        entity_id: 'exec-1',
        task_id: 'task-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(invalidateQueries).toHaveBeenCalled()
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['executions', 'exec-1'] }],
        [{ queryKey: ['tasks', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'detail'] }],
        [{ queryKey: ['tasks', 'task-1', 'executions'] }],
        [{ queryKey: ['tasks', 'task-1', 'diff'] }],
        [{ queryKey: ['agents'] }],
      ]),
    )
    expect(dispatch).not.toHaveBeenCalled()
  })

  it('invalidates task and project task list on task.status_changed', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'task.status_changed',
        entity_id: 'task-1',
        task_id: 'task-1',
        project_id: 'proj-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['tasks', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'detail'] }],
        [{ queryKey: ['projects', 'proj-1', 'tasks'] }],
      ]),
    )
  })

  it('ignores uncommitted Agent Chat streaming deltas until the ledger entry arrives', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'agent_chat.message_delta',
        entity_id: 'chat-1',
        chat_id: 'chat-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(dispatch).not.toHaveBeenCalled()
    expect(invalidateQueries).not.toHaveBeenCalled()
  })

  it('invalidates Agent Chat projections for committed ledger messages', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'agent_chat.message_created',
        entity_id: 'message-1',
        chat_id: 'chat-1',
        project_id: 'proj-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(dispatch).not.toHaveBeenCalled()
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['agent-chats'] }],
        [{ queryKey: ['agent-chats', 'chat-1'] }],
        [{ queryKey: ['agent-chats', 'chat-1', 'messages'] }],
        [{ queryKey: ['agent-chats', 'chat-1', 'turns'] }],
        [{ queryKey: ['agent-handoffs', 'proj-1'] }],
      ]),
    )
  })

  it('invalidates task review data for automated review results', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'review.passed',
        entity_id: 'review-1',
        task_id: 'task-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(dispatch).not.toHaveBeenCalled()
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['tasks', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'detail'] }],
        [{ queryKey: ['tasks', 'task-1', 'reviews'] }],
      ]),
    )
  })

  it('invalidates task workspace data for workspace events with task context', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'workspace.cleaned',
        entity_id: 'workspace-1',
        task_id: 'task-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(dispatch).not.toHaveBeenCalled()
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['tasks', 'task-1', 'workspace'] }],
        [{ queryKey: ['tasks', 'task-1', 'detail'] }],
      ]),
    )
  })

  it('invalidates active queries for reconciliation/resync events', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      {
        event_type: 'reconciliation.event',
        entity_id: 'task-1',
        timestamp: '2026-05-05T00:00:00Z',
      },
      queryClient,
      { dispatch },
    )
    expect(invalidateQueries).toHaveBeenCalledWith(
      expect.objectContaining({ refetchType: 'active' }),
    )
  })
})
