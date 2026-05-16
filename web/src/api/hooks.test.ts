import { afterEach, describe, expect, it, vi } from 'vitest'

import { getExecutionHookLogs, getExecutionLogs } from './hooks'
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
      new Response(JSON.stringify({ items: [], has_more: false }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    )

    await expect(getExecutionLogs('exec-1', { tail: 500 })).resolves.toEqual({
      items: [],
      has_more: false,
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
})
