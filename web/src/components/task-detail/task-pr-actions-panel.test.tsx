import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import type { PrExternalActionListResponse, ProjectMemberResponse, Task } from '@/types/generated'
import { TaskPrActionsPanel } from './task-pr-actions-panel'

const actions = vi.hoisted(() => ({
  query: {
    data: undefined as { pages: PrExternalActionListResponse[] } | undefined,
    isLoading: false,
    isError: false,
    hasNextPage: false,
    isFetchingNextPage: false,
    refetch: vi.fn(),
    fetchNextPage: vi.fn(),
  },
  decide: { mutateAsync: vi.fn(), isPending: false, variables: undefined },
}))

vi.mock('@/api/hooks', () => ({
  useTaskPrActionsQuery: () => actions.query,
  useDecidePrExternalAction: () => actions.decide,
}))

vi.mock('@/stores/auth', () => ({
  useAuthStore: (selector: (state: { user: { id: string; is_admin: boolean } }) => unknown) =>
    selector({ user: { id: 'user-1', is_admin: false } }),
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
const members = [{ user_id: 'user-1', role: 'owner' }] as ProjectMemberResponse[]

function actionResponse(): PrExternalActionListResponse {
  return {
    items: [
      {
        id: 'action-1',
        task_id: 'task-1',
        repo_id: 'repo-1',
        admission_id: 'admission-1',
        kind: 'push_update',
        state: 'proposed',
        authorization_mode: 'human_approval',
        gate_evaluation_id: null,
        subject_commit_sha: 'commit-1',
        expected_remote_head_sha: 'head-1',
        summary_artifact_id: 'artifact-1',
        body_digest: 'digest-1',
        proposal_id: 'proposal-1',
        decision_id: null,
        version: 4,
        created_at: '2026-11-16T12:00:00Z',
        updated_at: '2026-11-16T12:00:00Z',
        finished_at: null,
      },
    ],
    next_cursor: null,
  }
}

describe('TaskPrActionsPanel', () => {
  beforeEach(() => {
    actions.query.data = { pages: [actionResponse()] }
    actions.query.isLoading = false
    actions.query.isError = false
    actions.query.hasNextPage = false
    actions.query.refetch.mockReset()
    actions.query.fetchNextPage.mockReset()
    actions.decide.mutateAsync.mockReset()
    actions.decide.isPending = false
    actions.decide.variables = undefined
  })

  it('sends a versioned approval with an idempotency key', async () => {
    actions.decide.mutateAsync.mockResolvedValue({})
    render(<TaskPrActionsPanel task={task} members={members} />)

    fireEvent.click(screen.getByRole('button', { name: 'Approve action' }))

    await waitFor(() => expect(actions.decide.mutateAsync).toHaveBeenCalled())
    const request = actions.decide.mutateAsync.mock.calls[0]?.[0]
    expect(request).toEqual(
      expect.objectContaining({
        taskId: 'task-1',
        actionId: 'action-1',
        decision: 'approve',
        body: expect.objectContaining({
          expected_version: 4,
          note: null,
          idempotency_key: expect.any(String),
        }),
      }),
    )
    expect(screen.getByText(/Merging is always a human operation outside Forge/)).toBeTruthy()
  })

  it('reloads the list on an action version conflict', async () => {
    actions.decide.mutateAsync.mockRejectedValue(
      new ApiError(JSON.stringify({ code: 'pr_action.version_conflict', message: 'stale' }), 409),
    )
    actions.query.refetch.mockResolvedValue({ data: { pages: [actionResponse()] } })
    render(<TaskPrActionsPanel task={task} members={members} />)

    fireEvent.click(screen.getByRole('button', { name: 'Reject action' }))

    expect((await screen.findByRole('status')).textContent).toMatch(
      /changed while you were reading/,
    )
    expect(actions.query.refetch).toHaveBeenCalled()
  })

  it('does not offer decisions for a non-owner', () => {
    render(<TaskPrActionsPanel task={task} members={[]} />)
    expect(screen.queryByRole('button', { name: 'Approve action' })).toBeNull()
    expect(screen.getByText(/Only a Project owner or admin/)).toBeTruthy()
  })
})
