import { describe, expect, it } from 'vitest'
import type { Repo } from '@/types/generated'
import { emptyRepoForm } from '@/components/settings/RepoForm'
import {
  repoFormFromRepo,
  repoPrProviderConfigFromForm,
} from '@/components/settings/project-settings-utils'

describe('repository settings form mapping', () => {
  it('includes the selected GitHub credential in create-time provider config', () => {
    expect(
      repoPrProviderConfigFromForm({
        ...emptyRepoForm,
        work_mode: 'pull_request',
        credential_id: 'scm-handle-1',
        pr_polling_interval_seconds: '45',
      }),
    ).toEqual({
      base_url: null,
      polling_interval_seconds: 45,
      credential_id: 'scm-handle-1',
    })
  })

  it('preselects and labels the credential already referenced by the repo', () => {
    const repo: Repo = {
      id: 'repo-1',
      project_id: 'project-1',
      name: 'Forge',
      local_path: null,
      remote_url: 'https://github.com/acme/forge.git',
      default_branch: 'main',
      work_mode: 'pull_request',
      pr_provider: 'github',
      pr_provider_status: {
        provider_type: 'github',
        has_credential: true,
        credential_handle_id: 'scm-handle-1',
        credential_label: 'Forge GitHub',
        polling_interval_seconds: 60,
      },
      created_at: '2026-11-16T10:00:00Z',
      updated_at: '2026-11-16T10:00:00Z',
    }

    expect(repoFormFromRepo(repo)).toMatchObject({
      credential_id: undefined,
      credential_handle_id: 'scm-handle-1',
      credential_label: 'Forge GitHub',
      credential_connected: true,
    })
  })
})
