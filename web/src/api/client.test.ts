import { afterEach, describe, expect, it, vi } from 'vitest'

import { addDependency, apiFetch, removeDependency } from './client'

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

  it('fences Task dependency writes with the current Project graph version and replay key', async () => {
    const fetchMock = vi.spyOn(window, 'fetch').mockResolvedValue(
      new Response(null, { status: 204, statusText: 'No Content' }),
    )

    await addDependency('task id', 'prerequisite id', 4, 'add-key')
    await removeDependency('task id', 'prerequisite id', 5, 'remove-key')

    const addCall = fetchMock.mock.calls[0]
    expect(String(addCall?.[0])).toContain('/tasks/task%20id/dependencies')
    expect(JSON.parse((addCall?.[1] as RequestInit).body as string)).toEqual({
      depends_on_id: 'prerequisite id',
      expected_graph_version: 4,
      idempotency_key: 'add-key',
    })
    const removeCall = fetchMock.mock.calls[1]
    expect((removeCall?.[1] as RequestInit).method).toBe('DELETE')
    expect(String(removeCall?.[0])).toContain(
      '/tasks/task%20id/dependencies/prerequisite%20id',
    )
    expect(JSON.parse((removeCall?.[1] as RequestInit).body as string)).toEqual({
      expected_graph_version: 5,
      idempotency_key: 'remove-key',
    })
  })
})
