import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAuthStore } from '@/stores/auth'
import { ReposTab } from '@/components/settings/ReposTab'
import type { Project, Repo } from '@/types/generated'

const state = vi.hoisted(() => ({
  reposQuery: { data: { items: [] as Repo[] }, isLoading: false, isError: false, refetch: vi.fn() },
  membersQuery: {
    data: [
      {
        id: 'member-1',
        user_id: 'user-1',
        email: 'owner@example.com',
        display_name: null,
        role: 'owner',
        created_at: '',
      },
    ],
  },
  daemonsQuery: { data: { items: [] as Array<{ id: string; status: string }> } },
  createMutation: { mutateAsync: vi.fn(), isPending: false },
  updateMutation: { mutateAsync: vi.fn(), isPending: false, mutate: vi.fn() },
  syncMutation: { isPending: false, mutate: vi.fn() },
  updateRepoIds: [] as string[],
  scmCredentials: { list: vi.fn() },
}))

vi.mock('@/api/hooks', () => ({
  useMembersQuery: () => state.membersQuery,
  useReposQuery: () => state.reposQuery,
  useCreateRepo: () => state.createMutation,
  useDaemonsQuery: () => state.daemonsQuery,
  useUpdateRepo: (repoId: string) => {
    state.updateRepoIds.push(repoId)
    return state.updateMutation
  },
  useSyncRepo: () => state.syncMutation,
}))

vi.mock('@/api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/api/client')>()
  return { ...actual, listScmCredentials: state.scmCredentials.list }
})

vi.mock('@/components/settings/IntegrationsTab', () => ({ IntegrationsTab: () => null }))
vi.mock('@/components/settings/RepoPrFeedbackPolicyEditor', () => ({
  RepoPrFeedbackPolicyEditor: () => null,
}))

const primaryRepo: Repo = {
  id: 'repo-primary',
  project_id: 'project-1',
  name: 'Primary app',
  local_path: '/work/app',
  remote_url: 'https://github.com/acme/app.git',
  default_branch: 'main',
  work_mode: 'direct_merge',
  pr_provider: null,
  pr_provider_status: null,
  created_at: '',
  updated_at: '',
}

const secondaryRepo: Repo = {
  id: 'repo-secondary',
  project_id: 'project-1',
  name: 'Shared library',
  local_path: '/work/shared-library',
  remote_url: 'https://github.com/acme/shared-library.git',
  default_branch: 'main',
  work_mode: 'pull_request',
  pr_provider: 'github',
  pr_provider_status: {
    provider_type: 'github',
    has_credential: true,
    credential_handle_id: 'credential-handle-1',
    credential_label: 'Forge SCM',
    polling_interval_seconds: 60,
  },
  created_at: '',
  updated_at: '',
}

const project: Project = {
  id: 'project-1',
  name: 'Project One',
  project_hooks: [],
  primary_repo_id: primaryRepo.id,
  owner_id: 'user-1',
  created_at: '',
  updated_at: '',
  paused_at: null,
  paused: false,
  version: 1,
}

function renderReposTab() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <ReposTab
        project={project}
        projectId={project.id}
        isUpdatingPrimaryRepo={false}
        onUpdatePrimaryRepo={vi.fn()}
      />
    </QueryClientProvider>,
  )
}

describe('ReposTab repositories', () => {
  beforeEach(() => {
    state.reposQuery = {
      data: { items: [primaryRepo, secondaryRepo] },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    }
    state.membersQuery = {
      data: [
        {
          id: 'member-1',
          user_id: 'user-1',
          email: 'owner@example.com',
          display_name: null,
          role: 'owner',
          created_at: '',
        },
      ],
    }
    state.daemonsQuery = { data: { items: [] } }
    state.createMutation.mutateAsync.mockReset().mockResolvedValue({})
    state.updateMutation.mutateAsync.mockReset().mockResolvedValue({})
    state.updateMutation.mutate.mockReset()
    state.syncMutation.mutate.mockReset()
    state.updateRepoIds = []
    state.scmCredentials.list.mockReset().mockResolvedValue({
      items: [
        {
          id: 'credential-1',
          provider: 'github',
          label: 'Forge SCM',
          credential_method: 'api_key',
          status: 'configured',
          base_url: null,
          provider_account_id: null,
          used_by: [],
          last_used_at: null,
          version: 1,
          created_at: '',
          updated_at: '',
        },
      ],
      cli_runtimes: [],
    })
    useAuthStore.setState({
      accessToken: 't',
      refreshToken: 'r',
      user: {
        id: 'user-1',
        email: 'owner@example.com',
        display_name: null,
        is_admin: false,
        created_at: '',
      },
    })
  })

  afterEach(() => useAuthStore.getState().clearAuth())

  it('shows the primary repo and every additional repo with its provider status', () => {
    renderReposTab()

    expect(screen.getByText('Primary', { exact: true })).toBeTruthy()
    expect(screen.getByText('Primary app', { selector: 'p' })).toBeTruthy()
    expect(screen.getByText('Shared library')).toBeTruthy()
    expect(screen.getByText(secondaryRepo.remote_url)).toBeTruthy()
    expect(screen.getByText(secondaryRepo.local_path as string)).toBeTruthy()
    expect(screen.getAllByText('Pull request').length).toBeGreaterThan(0)
    expect(screen.getByText('github')).toBeTruthy()
    expect(screen.getByText('Credential connected: Forge SCM')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Sync' })).toBeTruthy()
    expect(screen.getByText('main')).toBeTruthy()
  })

  it('edits a non-primary repo through the existing repository form', async () => {
    renderReposTab()

    const editButtons = screen.getAllByRole('button', { name: 'Edit' })
    fireEvent.click(editButtons[1] as HTMLButtonElement)

    expect(screen.getByRole('heading', { name: 'Edit Repository' })).toBeTruthy()
    expect(screen.getByLabelText(/^Remote URL/).getAttribute('value')).toBe(
      secondaryRepo.remote_url,
    )
    fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: 'Shared library edited' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(state.updateMutation.mutateAsync).toHaveBeenCalled())
    expect(state.updateRepoIds.includes(secondaryRepo.id)).toBe(true)
    expect(state.updateMutation.mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'Shared library edited', work_mode: 'pull_request' }),
    )
  })

  it('adds a repository with work mode, provider, and SCM credential when a primary exists', async () => {
    renderReposTab()

    fireEvent.click(screen.getByRole('button', { name: /Add repository/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Add remote repository' }))
    expect(screen.getByRole('heading', { name: 'Add Remote Repository' })).toBeTruthy()

    fireEvent.change(screen.getByLabelText('Remote URL'), {
      target: { value: 'https://github.com/acme/new-repo.git' },
    })
    fireEvent.click(screen.getByLabelText('Work mode'))
    fireEvent.click(screen.getByRole('option', { name: 'Pull request' }))
    await waitFor(() => expect(state.scmCredentials.list).toHaveBeenCalled())
    fireEvent.click(screen.getByLabelText('GitHub SCM credential'))
    fireEvent.click(await screen.findByRole('option', { name: 'Forge SCM' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(state.createMutation.mutateAsync).toHaveBeenCalled())
    expect(state.createMutation.mutateAsync).toHaveBeenCalledWith({
      remote_url: 'https://github.com/acme/new-repo.git',
      name: null,
      local_path: null,
      default_branch: 'main',
      work_mode: 'pull_request',
      pr_provider: 'github',
      pr_provider_config: {
        base_url: null,
        polling_interval_seconds: 60,
        credential_id: 'credential-1',
      },
    })
  })
})
