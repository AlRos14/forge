import { afterEach, describe, expect, it, vi } from 'vitest'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook } from '@testing-library/react'
import { createElement, type ReactNode } from 'react'

import { getExecutionHookLogs, getExecutionLogs, useTransitionTaskLifecycle } from './hooks'
import { qk } from './query-keys'
import { useAuthStore } from '@/stores/auth'

describe('execution log API helpers', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    useAuthStore.getState().clearAuth()
    localStorage.clear()
  })

  it('loads execution logs through the authenticated API client', async () => {
    useAuthStore.setState({ accessToken: 'access-token', refreshToken: 'refresh-token' })
    const fetchMock = vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ items: [], has_more: false, next_sequence: null }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    )

    await expect(getExecutionLogs('exec-1', { tail: 500 })).resolves.toEqual({
      items: [],
      has_more: false,
      next_sequence: null,
    })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect((url as URL).pathname).toBe('/api/v1/executions/exec-1/logs')
    expect((url as URL).searchParams.get('tail')).toBe('500')
    expect((init?.headers as Headers).get('authorization')).toBe('Bearer access-token')
  })

  it('loads hook logs without duplicating the API prefix', async () => {
    useAuthStore.setState({ accessToken: 'access-token', refreshToken: 'refresh-token' })
    const fetchMock = vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    )

    await expect(getExecutionHookLogs('exec-1')).resolves.toEqual([])

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect((url as URL).pathname).toBe('/api/v1/executions/exec-1/hook-logs')
    expect((init?.headers as Headers).get('authorization')).toBe('Bearer access-token')
  })

  it('invalidates Task dispatch preflight after a lifecycle transition', async () => {
    const fetchMock = vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(
        JSON.stringify({
          task_id: 'task-1',
          lifecycle: {},
          transition_id: 'transition-1',
          gate_evaluation_id: null,
          replayed: false,
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      ),
    )
    const queryClient = new QueryClient()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue()
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: queryClient }, children)

    const { result } = renderHook(() => useTransitionTaskLifecycle(), { wrapper })
    await act(async () => {
      await result.current.mutateAsync({
        taskId: 'task-1',
        toState: 'ready',
        expectedLifecycleVersion: 4,
        idempotencyKey: 'transition-key',
      })
    })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(invalidate).toHaveBeenCalledWith({ queryKey: qk.taskDispatch('task-1') })
    queryClient.clear()
  })
})
