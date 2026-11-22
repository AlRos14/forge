import type { ReactNode } from 'react'
import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  ExternalFeedbackItemListResponse,
  PrReworkListResponse,
  Task,
} from '@/types/generated'
import { TaskPrFeedbackPanel } from './task-pr-feedback-panel'

const queries = vi.hoisted(() => ({
  feedback: {
    data: undefined as { pages: ExternalFeedbackItemListResponse[] } | undefined,
    isLoading: false,
    isError: false,
    hasNextPage: false,
    isFetchingNextPage: false,
    refetch: vi.fn(),
    fetchNextPage: vi.fn(),
  },
  rework: {
    data: undefined as { pages: PrReworkListResponse[] } | undefined,
    isLoading: false,
    isError: false,
    hasNextPage: false,
    isFetchingNextPage: false,
    refetch: vi.fn(),
    fetchNextPage: vi.fn(),
  },
}))

vi.mock('@/api/hooks', () => ({
  useTaskPrFeedbackQuery: () => queries.feedback,
  useTaskPrReworkQuery: () => queries.rework,
}))

vi.mock('@tanstack/react-router', () => ({
  Link: ({
    params,
    children,
    className,
  }: {
    params: { taskId: string }
    children: ReactNode
    className?: string
  }) => (
    <a href={`/tasks/${params.taskId}`} className={className}>
      {children}
    </a>
  ),
}))

const task = {
  id: 'task-1',
  project_id: 'project-1',
  repo_id: null,
  repositories: [],
  repository_set_revision: 0,
  repository_set_digest: null,
  repository_merge_batches: [],
} as unknown as Task

describe('TaskPrFeedbackPanel', () => {
  beforeEach(() => {
    queries.feedback.data = undefined
    queries.feedback.isLoading = false
    queries.feedback.isError = false
    queries.feedback.hasNextPage = false
    queries.feedback.refetch.mockReset()
    queries.feedback.fetchNextPage.mockReset()
    queries.rework.data = undefined
    queries.rework.isLoading = false
    queries.rework.isError = false
    queries.rework.hasNextPage = false
    queries.rework.refetch.mockReset()
    queries.rework.fetchNextPage.mockReset()
  })

  it('shows feedback trust, head state, disposition, follow-up and decision links, plus rework state', () => {
    queries.feedback.data = {
      pages: [
        {
          items: [
            {
              id: 'feedback-1',
              task_id: 'task-1',
              admission_id: 'admission-1',
              provider_item_id: 'review-1',
              kind: 'review_comment',
              author_login: 'reviewer',
              author_association: 'MEMBER',
              review_state: 'commented',
              path: 'src/lib.rs',
              line: 14,
              commit_sha: 'abc123',
              body: 'Please handle this boundary case.',
              body_truncated: false,
              body_digest: 'digest-1',
              provider_url: 'https://github.com/acme/forge/pull/1#review-1',
              trust_class: 'trusted',
              head_state: 'current',
              disposition: 'propose_task',
              dismiss_reason: null,
              scope_classification: 'scope_change',
              human_decision_id: 'decision-1',
              followup_task_id: 'followup-1',
              ingested_at: '2026-11-16T12:00:00Z',
            },
          ],
          next_cursor: 'feedback-cursor',
        },
      ],
    }
    queries.feedback.hasNextPage = true
    queries.rework.data = {
      pages: [
        {
          items: [
            {
              id: 'rework-1',
              task_id: 'task-1',
              task_merge_operation_id: 'merge-1',
              publish_operation_id: 'publish-1',
              metadata_id: 'metadata-1',
              request_id: 'request-1',
              triage_set_digest: 'digest-set',
              provider_pr_id: '42',
              superseded_head_sha: 'head-1',
              state: 'superseded_for_rework',
              readmission_merge_operation_id: null,
              convergence_decision_id: null,
              version: 1,
              created_at: '2026-11-16T12:00:00Z',
              updated_at: '2026-11-16T12:00:00Z',
            },
          ],
          next_cursor: null,
        },
      ],
    }

    render(<TaskPrFeedbackPanel task={task} />)

    expect(screen.getByText('trusted')).toBeTruthy()
    expect(screen.getByText('Current head')).toBeTruthy()
    expect(screen.getByText('propose task')).toBeTruthy()
    expect(screen.getByText('Please handle this boundary case.')).toBeTruthy()
    expect(screen.getByRole('link', { name: /Follow-up Task/ }).getAttribute('href')).toBe(
      '/tasks/followup-1',
    )
    expect(screen.getByRole('link', { name: /Human decision/ }).getAttribute('href')).toBe(
      '#human-decision-decision-1',
    )
    expect(screen.getByText('superseded for rework')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Load older feedback' }))
    expect(queries.feedback.fetchNextPage).toHaveBeenCalled()
  })
})
