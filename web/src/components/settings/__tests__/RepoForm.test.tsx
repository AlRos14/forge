import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { connectScmCredential, listScmCredentials } from '@/api/client'
import { RepoForm, type RepoFormState } from '@/components/settings/RepoForm'
import type { Daemon, ProviderEntryResponse } from '@/types/generated'

const scmCredentialApi = vi.hoisted(() => ({ list: vi.fn(), connect: vi.fn() }))

vi.mock('@/api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/api/client')>()
  return {
    ...actual,
    listScmCredentials: scmCredentialApi.list,
    connectScmCredential: scmCredentialApi.connect,
  }
})

const localRepoForm: RepoFormState = {
  source_mode: 'local',
  name: 'forge',
  local_path: '/work/forge',
  remote_url: 'https://github.com/acme/forge.git',
  default_branch: 'main',
  work_mode: 'direct_merge',
  pr_provider: 'github',
  pr_base_url: '',
  credential_id: undefined,
  credential_connected: false,
  pr_polling_interval_seconds: '60',
}

const connectedCredential: ProviderEntryResponse = {
  id: 'scm-credential-1',
  provider: 'github',
  label: 'Forge GitHub',
  credential_method: 'api_key',
  status: 'configured',
  base_url: null,
  provider_account_id: null,
  used_by: [],
  last_used_at: null,
  version: 1,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
}

const daemons: Daemon[] = [
  {
    id: 'daemon-1',
    machine_id: 'machine-1',
    hostname: 'alpha',
    os: 'linux',
    arch: 'x64',
    agent_version: null,
    status: 'online',
    last_report_at: null,
    detected_clis: [],
    labels: {},
    owner_id: null,
    visibility: 'global',
    version: 1,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
  {
    id: 'daemon-2',
    machine_id: 'machine-2',
    hostname: 'beta',
    os: 'linux',
    arch: 'x64',
    agent_version: null,
    status: 'online',
    last_report_at: null,
    detected_clis: [],
    labels: {},
    owner_id: null,
    visibility: 'global',
    version: 1,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  },
]

function RepoFormHarness({
  initialForm,
  onSubmit,
}: {
  initialForm: RepoFormState
  onSubmit: (form: RepoFormState) => void
}) {
  const [form, setForm] = useState(initialForm)

  return (
    <RepoForm
      form={form}
      open
      pending={false}
      daemons={daemons}
      daemonId={undefined}
      onCancel={() => {}}
      onDaemonChange={() => {}}
      onSubmit={onSubmit}
      onUpdate={setForm}
    />
  )
}

function renderRepoForm(initialForm: RepoFormState, onSubmit: (form: RepoFormState) => void) {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  })

  render(
    <QueryClientProvider client={queryClient}>
      <RepoFormHarness initialForm={initialForm} onSubmit={onSubmit} />
    </QueryClientProvider>,
  )
}

describe('RepoForm', () => {
  beforeEach(() => {
    vi.mocked(listScmCredentials).mockResolvedValue({ items: [connectedCredential], cli_runtimes: [] })
    vi.mocked(connectScmCredential).mockResolvedValue(connectedCredential)
  })

  it('allows saving a local repo remote URL without selecting a daemon', () => {
    const onSubmit = vi.fn()

    renderRepoForm(localRepoForm, onSubmit)

    const saveButton = screen.getByRole('button', { name: 'Save' }) as HTMLButtonElement
    expect(saveButton.disabled).toBe(false)

    fireEvent.change(screen.getByLabelText(/Remote URL/), {
      target: { value: 'https://github.com/acme/forge-renamed.git' },
    })
    fireEvent.click(saveButton)

    expect(onSubmit).toHaveBeenCalledWith({
      ...localRepoForm,
      remote_url: 'https://github.com/acme/forge-renamed.git',
      default_branch: 'main',
    })
  })

  it('lets the user select a connected GitHub credential for a pull request repo', () => {
    const onSubmit = vi.fn()
    renderRepoForm({ ...localRepoForm, work_mode: 'pull_request' }, onSubmit)

    fireEvent.click(screen.getByRole('button', { name: 'GitHub SCM credential' }))
    fireEvent.click(screen.getByRole('option', { name: 'Forge GitHub' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ credential_id: 'scm-credential-1', credential_connected: true }),
    )
  })

  it('connects a credential in the request body and clears the password input after submit', async () => {
    const onSubmit = vi.fn()
    const form = { ...localRepoForm, work_mode: 'pull_request' as const }
    renderRepoForm(form, onSubmit)

    fireEvent.click(screen.getByRole('button', { name: 'Connect a new GitHub credential' }))
    fireEvent.change(screen.getByLabelText('Credential label'), {
      target: { value: 'Forge GitHub' },
    })
    fireEvent.change(screen.getByLabelText('GitHub credential'), {
      target: { value: 'secret-value' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Connect credential' }))

    await waitFor(() =>
      expect(vi.mocked(connectScmCredential)).toHaveBeenCalledWith({
        provider: 'github',
        label: 'Forge GitHub',
        credential: 'secret-value',
      }),
    )
    await waitFor(() => expect(screen.queryByLabelText('GitHub credential')).toBeNull())
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ credential_id: 'scm-credential-1' }),
    )
    expect(onSubmit.mock.calls[0]?.[0]).not.toHaveProperty('credential')
  })
})
