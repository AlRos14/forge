import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { RepoForm, type RepoFormState } from '@/components/settings/RepoForm'
import type { Daemon } from '@/types/generated/api'

const localRepoForm: RepoFormState = {
  source_mode: 'local',
  name: 'forge',
  local_path: '/work/forge',
  remote_url: 'https://github.com/acme/forge.git',
  default_branch: 'main',
  work_mode: 'direct_merge',
  pr_provider: 'github',
  pr_base_url: '',
  pr_token: '',
  pr_polling_interval_seconds: '60',
}

const daemons: Daemon[] = [
  {
    id: 'daemon-1',
    machine_id: 'machine-1',
    hostname: 'alpha',
    os: 'linux',
    arch: 'x64',
    status: 'online',
    detected_clis: [],
    labels: {},
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
    status: 'online',
    detected_clis: [],
    labels: {},
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
})
