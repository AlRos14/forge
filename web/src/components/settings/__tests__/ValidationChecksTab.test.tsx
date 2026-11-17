import { render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAuthStore } from '@/stores/auth'
import { ValidationChecksTab } from '@/components/settings/ValidationChecksTab'

const state = vi.hoisted(() => ({ role: 'member' }))

vi.mock('@/api/hooks', () => ({
  useMembersQuery: () => ({ data: [{ user_id: 'u1', role: state.role }] }),
  useReposQuery: () => ({ data: { items: [] } }),
  useProjectValidationChecksQuery: () => ({
    isLoading: false,
    isError: false,
    data: {
      items: [
        {
          id: 'c1', project_id: 'p1', repo_id: null, check_key: 'unit', version: 2,
          command: 'cargo test', timeout_seconds: 600, env_policy: 'x', definition_digest: 'd',
          state: 'active', created_by: 'u1', created_at: '', retired_at: null,
        },
      ],
    },
  }),
  useCreateValidationCheck: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useRetireValidationCheck: () => ({ mutateAsync: vi.fn(), isPending: false }),
}))

describe('ValidationChecksTab permissions', () => {
  beforeEach(() => {
    useAuthStore.setState({
      accessToken: 't',
      refreshToken: 'r',
      user: { id: 'u1', email: 'a@b.c', display_name: null, is_admin: false, created_at: '' },
    })
  })
  afterEach(() => useAuthStore.getState().clearAuth())

  it('is read-only for members', () => {
    state.role = 'member'
    render(<ValidationChecksTab projectId="p1" />)
    expect(screen.getByText('unit@2')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Retire' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Publish version' })).toBeNull()
  })

  it('lets owners publish and retire', () => {
    state.role = 'owner'
    render(<ValidationChecksTab projectId="p1" />)
    expect(screen.getByRole('button', { name: 'Retire' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Publish version' })).toBeTruthy()
  })
})
