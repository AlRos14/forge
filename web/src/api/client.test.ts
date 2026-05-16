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
