import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { ReviewExecutionResponse, Task } from '@/types/generated'
import { TaskReworkPanel } from './task-rework-panel'

const mocks = vi.hoisted(() => ({ request: vi.fn() }))

vi.mock('@/api/hooks', () => ({
  useRequestRework: () => ({ mutateAsync: mocks.request, isPending: false }),
  useTaskReworkAutomationQuery: () => ({ data: { items: [] } }),
}))

const task: Task = {
  id: 'task-1',
  project_id: 'project-1',
  repo_id: null,
  repositories: [],
  repository_set_revision: 0,
  repository_set_digest: null,
  repository_merge_batches: [],
  parent_task_id: null,
  title: 'Task',
  description: null,
  version: 4,
  lifecycle: {
    task_id: 'task-1',
    state: 'active',
    version: 3,
    reason_kind: null,
    reason_ref: null,
    created_at: '2026-11-20T00:00:00Z',
    updated_at: '2026-11-20T00:00:00Z',
  },
  priority: 0,
  board_position: 0,
  subtask_order: null,
  task_roles: [
    {
      id: 'role-implementer',
      task_id: 'task-1',
      role: 'implementer',
      coordination_mode: null,
      policy: {},
      configuration: { schema_version: 1, display_label: null, capabilities: [], rework: null },
      version: 1,
      members: [],
      created_at: '2026-11-20T00:00:00Z',
      updated_at: '2026-11-20T00:00:00Z',
    },
    {
      id: 'role-reviewer',
      task_id: 'task-1',
      role: 'reviewer',
      coordination_mode: null,
      policy: {},
      configuration: {
        schema_version: 1,
        display_label: null,
        capabilities: ['formal_review', 'request_rework'],
        rework: {
          mode: 'manual',
          allowed_destination_role_ids: ['role-implementer'],
          next_review_role_id: 'role-reviewer',
          correction_actor: { kind: 'original_contributor' },
          review_actor: { kind: 'human', id: 'reviewer-1' },
        },
      },
      version: 1,
      members: [
        {
          id: 'membership-reviewer-1',
          task_role_id: 'role-reviewer',
          status: 'active',
          actor_ref: { kind: 'human', id: 'reviewer-1' },
          version: 1,
          created_at: '2026-11-20T00:00:00Z',
          updated_at: '2026-11-20T00:00:00Z',
          ended_at: null,
        },
      ],
      created_at: '2026-11-20T00:00:00Z',
      updated_at: '2026-11-20T00:00:00Z',
    },
  ],
  execution_observability: {
    execution_count: 0,
    total_runtime_seconds: 0,
    total_input_tokens: 0,
    total_output_tokens: 0,
    total_cache_read_tokens: 0,
    total_cache_write_tokens: 0,
    total_tokens: 0,
    total_cost_usd: null,
  },
  workspace: null,
  external_issue_number: null,
  external_issue_url: null,
  created_at: '2026-11-20T00:00:00Z',
  updated_at: '2026-11-20T00:00:00Z',
}

const review: ReviewExecutionResponse = {
  execution: {
    id: 'execution-review-1',
    task_id: 'task-1',
    actor_ref: { kind: 'agent', id: 'agent-reviewer-1' },
    role: 'reviewer',
    status: 'completed',
    parent_execution_id: null,
    harness_session_id: null,
    prompt: null,
    summary: null,
    logs_path: null,
    before_sha: null,
    after_sha: null,
    error: null,
    stop_reason: null,
    stopped_by: null,
    resume_policy: null,
    stopped_at: null,
    executor_config_snapshot: null,
    workspace_id: null,
    work_unit_id: 'unit-review-1',
    work_unit_version: 1,
    repository_results: [],
    usage: null,
    account_usage: null,
    created_at: '2026-11-20T00:00:00Z',
    updated_at: '2026-11-20T00:00:00Z',
  },
  report: {
    id: 'artifact-review-1',
    task_id: 'task-1',
    kind: 'review_report',
    storage_kind: 'inline',
    content: JSON.stringify({
      verdict: 'request_changes',
      subject: {
        reviewed_work_unit_id: 'unit-implementation',
        reviewed_work_unit_version: 2,
      },
    }),
    metadata: {},
    digest: null,
    producer: {
      kind: 'execution',
      execution_id: 'execution-review-1',
      actor: { kind: 'agent', id: 'agent-reviewer-1' },
    },
    created_at: '2026-11-20T00:00:00Z',
  },
}

describe('Task rework panel retry identity', () => {
  it('reuses one idempotency key when retrying the same exact-subject request', async () => {
    mocks.request
      .mockReset()
      .mockRejectedValueOnce(new Error('response was lost'))
      .mockResolvedValue({ replayed: true, rounds_remaining: 7 })
    const onChanged = vi.fn().mockResolvedValue(undefined)

    render(<TaskReworkPanel task={task} review={review} onChanged={onChanged} />)

    const requestButton = screen.getByRole('button', {
      name: 'Request correction + new review',
    })
    fireEvent.click(requestButton)
    await waitFor(() => expect(mocks.request).toHaveBeenCalledTimes(1))
    fireEvent.click(requestButton)
    await waitFor(() => expect(mocks.request).toHaveBeenCalledTimes(2))

    const firstBody = mocks.request.mock.calls[0][0].body
    const retryBody = mocks.request.mock.calls[1][0].body
    expect(firstBody).toMatchObject({
      review_report_artifact_id: 'artifact-review-1',
      reviewed_work_unit_id: 'unit-implementation',
      reviewed_work_unit_version: 2,
      correction_role_id: 'role-implementer',
    })
    expect(retryBody).toEqual(firstBody)
    expect(retryBody.idempotency_key).toBeTruthy()
  })
})
