import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { Task } from '@/types/generated'
import { TaskRepositoryPanel } from './task-repository-panel'

describe('TaskRepositoryPanel', () => {
  it('multirepo_pr_api_mcp_web_show_repo_identity', () => {
    const task = {
      repository_set_revision: 3,
      repository_set_digest: null,
      repositories: [
        {
          repo_id: 'repo-primary',
          role: 'primary',
          position: 0,
          change_state: 'changed',
          integration: {
            repo_id: 'repo-primary',
            integration_id: 'integration-primary',
            outcome: 'success',
            before_sha: 'a'.repeat(40),
            after_sha: 'b'.repeat(40),
          },
          merge: {
            batch_id: 'batch-1',
            repo_id: 'repo-primary',
            state: 'merged',
            integration_id: 'integration-primary',
            task_merge_operation_id: 'merge-primary',
            pr_metadata_id: 'metadata-primary',
            before_sha: 'a'.repeat(40),
            after_sha: 'b'.repeat(40),
          },
          pull_request: null,
        },
        {
          repo_id: 'repo-affected',
          role: 'affected',
          position: 1,
          change_state: 'changed',
          integration: null,
          merge: {
            batch_id: 'batch-1',
            repo_id: 'repo-affected',
            state: 'open',
            integration_id: 'integration-affected',
            task_merge_operation_id: 'merge-affected',
            pr_metadata_id: 'metadata-affected',
            before_sha: 'c'.repeat(40),
            after_sha: 'd'.repeat(40),
          },
          pull_request: {
            repo_id: 'repo-affected',
            admission_id: 'admission-affected',
            pr_metadata_id: 'metadata-affected',
            provider_type: 'github',
            provider_pr_id: '42',
            pr_url: 'https://example.test/org/repo-affected/pull/42',
            pr_state: 'open',
            merge_status: 'unmerged',
            admission_status: 'open',
          },
        },
      ],
      repository_merge_batches: [
        {
          id: 'batch-1',
          task_id: 'task-1',
          gate_evaluation_id: 'gate-eval-1',
          repository_set_revision: 3,
          state: 'open',
          version: 2,
          repositories: [
            {
              batch_id: 'batch-1',
              repo_id: 'repo-primary',
              state: 'merged',
              integration_id: 'integration-primary',
              task_merge_operation_id: 'merge-primary',
              pr_metadata_id: 'metadata-primary',
              before_sha: 'a'.repeat(40),
              after_sha: 'b'.repeat(40),
            },
            {
              batch_id: 'batch-1',
              repo_id: 'repo-affected',
              state: 'open',
              integration_id: 'integration-affected',
              task_merge_operation_id: 'merge-affected',
              pr_metadata_id: 'metadata-affected',
              before_sha: 'c'.repeat(40),
              after_sha: 'd'.repeat(40),
            },
          ],
          created_at: '',
          updated_at: '',
        },
      ],
    } as unknown as Task

    render(<TaskRepositoryPanel task={task} />)

    expect(screen.getByText('Task repositories · revision 3')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: /Pull request · open · unmerged/ })).toHaveAttribute(
      'href',
      'https://example.test/org/repo-affected/pull/42',
    )
    expect(screen.getByText('repo-affected: open · dddddddddddd')).toBeInTheDocument()
  })
})
