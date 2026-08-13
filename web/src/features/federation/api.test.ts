import { afterEach, describe, expect, it, vi } from 'vitest'
import { getContextManifest, listContextManifests } from './api'

const { apiFetch } = vi.hoisted(() => ({ apiFetch: vi.fn() }))

vi.mock('@/api/client', () => ({ apiFetch }))

describe('context-manifest API adapter', () => {
  afterEach(() => {
    apiFetch.mockReset()
  })

  it('uses the identity-scoped authorized discovery route', async () => {
    apiFetch.mockResolvedValueOnce({ items: [{ id: 'manifest-1' }], has_more: false })

    await expect(
      listContextManifests({ identity_id: 'identity-1', context_scope_id: 'scope-1' }),
    ).resolves.toEqual([{ id: 'manifest-1' }])
    expect(apiFetch).toHaveBeenCalledWith('/agents/identity-1/context-manifests', {
      search: { context_scope_id: 'scope-1', limit: 50 },
    })
  })

  it('keeps detail lookup scoped by identity and context scope', async () => {
    apiFetch.mockResolvedValueOnce({ id: 'manifest-1' })

    await expect(
      getContextManifest('manifest-1', {
        identity_id: 'identity-1',
        context_scope_id: 'scope-1',
      }),
    ).resolves.toEqual({ id: 'manifest-1' })
    expect(apiFetch).toHaveBeenCalledWith('/context-manifests/manifest-1', {
      search: { identity_id: 'identity-1', context_scope_id: 'scope-1' },
    })
  })
})
