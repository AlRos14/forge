import { afterEach, describe, expect, it, vi } from 'vitest'

import { apiFetch } from './client'

describe('apiFetch', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('returns undefined for successful empty responses', async () => {
    vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(null, {
        status: 201,
        statusText: 'Created',
      }),
    )

    await expect(
      apiFetch<void>('/tasks/task-id/dependencies', { method: 'POST' }),
    ).resolves.toBeUndefined()
  })

  it('does not send application/json when the request has no body', async () => {
    const fetchMock = vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(null, {
        status: 204,
        statusText: 'No Content',
      }),
    )

    await apiFetch<void>('/tasks/task-id/cancel', { method: 'POST' })

    const init = fetchMock.mock.calls[0]?.[1] as RequestInit
    const headers = new Headers(init.headers)
    expect(headers.get('content-type')).toBeNull()
  })

  it('sets application/json when sending a JSON body', async () => {
    const fetchMock = vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    )

    await apiFetch<{ ok: boolean }>('/tasks/task-id/cancel', {
      method: 'POST',
      body: JSON.stringify({}),
    })

    const init = fetchMock.mock.calls[0]?.[1] as RequestInit
    const headers = new Headers(init.headers)
    expect(headers.get('content-type')).toBe('application/json')
  })

  it('parses successful JSON responses', async () => {
    vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    )

    await expect(apiFetch<{ ok: boolean }>('/status')).resolves.toEqual({ ok: true })
  })
})
