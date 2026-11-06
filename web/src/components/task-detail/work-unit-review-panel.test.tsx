import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { Task, WorkUnitResponse } from '@/types/generated'
import { WorkUnitHumanReviewPanel } from './work-unit-review-panel'

const mocks = vi.hoisted(() => ({ submit: vi.fn() }))

vi.mock('@/api/hooks', () => ({
  useSubmitWorkUnitReviewReport: () => ({ mutateAsync: mocks.submit, isPending: false }),
}))

function task(): Task {
  return {
    id: 'task-1',
    project_id: 'project-1',
    task_roles: [
      {
        id: 'role-reviewer',
        task_id: 'task-1',
        role: 'reviewer',
        coordination_mode: 'independent',
        policy: {},
        version: 1,
        members: [
          {
            id: 'membership-1',
            task_role_id: 'role-reviewer',
            actor_ref: { kind: 'human', id: 'user-1' },
            status: 'active',
            version: 1,
            created_at: '',
            updated_at: '',
            ended_at: null,
          },
        ],
        created_at: '',
        updated_at: '',
      },
    ],
  } as unknown as Task
}

function reviewUnit(): WorkUnitResponse {
  return {
    id: 'unit-review',
    task_id: 'task-1',
    parent_work_unit_id: null,
    title: 'Review implementation',
    scope: 'Check the implementation against the exact result.',
    execution_instructions: null,
    status: 'open',
    role: 'reviewer',
    assigned_actor: { kind: 'human', id: 'user-1' },
    requires_integration: false,
    provenance: null,
    created_by: { kind: 'human', id: 'user-1' },
    version: 3,
    created_at: '',
    updated_at: '',
    readiness: {
      runnable: true,
      ready_for_allocation: false,
      active_execution_ids: [],
      unsatisfied_dependency_ids: [],
      awaiting_integration: false,
      awaiting_acceptance: false,
    },
    review_subject: {
      id: 'unit-review',
      review_work_unit_id: 'unit-review',
      reviewed_work_unit_id: 'unit-implementation',
      reviewed_work_unit_version: 2,
      result_execution_id: 'execution-2',
      result_artifact_id: 'artifact-2',
      result_commit_sha: 'abc123',
      result_workspace_id: 'workspace-1',
      workspace_snapshot_digest: 'snapshot-1',
    },
  } as WorkUnitResponse
}

describe('Human WorkUnit review report', () => {
  it('submits the report against the exact frozen WorkUnit version', async () => {
    mocks.submit.mockReset().mockResolvedValue({
      review_execution: { execution: { id: 'review-execution-1', task_id: 'task-1' } },
      report: { id: 'report-1' },
    })

    render(
      <WorkUnitHumanReviewPanel
        task={task()}
        unit={reviewUnit()}
        currentUserId="user-1"
      />,
    )

    expect(screen.getByText(/WorkUnit unit-implementation v2/)).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Human review verdict'), {
      target: { value: 'request_changes' },
    })
    fireEvent.change(screen.getByLabelText('Human review summary'), {
      target: { value: 'The implementation misses one acceptance point.' },
    })
    fireEvent.change(screen.getByLabelText('Requested changes'), {
      target: { value: 'Handle the empty response case.' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Submit Human ReviewReport' }))

    await waitFor(() => expect(mocks.submit).toHaveBeenCalledTimes(1))
    expect(mocks.submit.mock.calls[0][0]).toMatchObject({
      workUnitId: 'unit-review',
      body: {
        expected_version: 3,
        report: {
          verdict: 'request_changes',
          summary: 'The implementation misses one acceptance point.',
          findings: ['Handle the empty response case.'],
          reviewed_work_unit_id: 'unit-implementation',
          reviewed_work_unit_version: 2,
        },
      },
    })
  })

  it('does not offer the form when the WorkUnit is assigned to someone else', () => {
    render(
      <WorkUnitHumanReviewPanel
        task={task()}
        unit={{ ...reviewUnit(), assigned_actor: { kind: 'human', id: 'user-2' } }}
        currentUserId="user-1"
      />,
    )

    expect(screen.queryByRole('button', { name: 'Submit Human ReviewReport' })).toBeNull()
  })
})
