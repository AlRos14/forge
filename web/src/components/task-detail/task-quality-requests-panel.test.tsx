import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type {
  Task,
  TaskCoordinationOverviewResponse,
  TaskQualityRequestResponse,
} from '@/types/generated'
import { TaskQualityRequestsPanel } from './task-quality-requests-panel'

const quality = vi.hoisted(() => ({
  items: [] as TaskQualityRequestResponse[],
}))

vi.mock('@/api/hooks', () => ({
  useTaskQualityRequestsQuery: () => ({
    data: { items: quality.items },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useCreateTaskQualityRequest: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useResolveTaskQualityRequest: () => ({ mutateAsync: vi.fn(), isPending: false }),
}))

const task = {
  id: 'task-1',
  project_id: 'project-1',
  title: 'Task',
  description: null,
  version: 1,
} as unknown as Task

const coordination = {
  quality_request_count: 3,
  max_quality_requests: 8,
  team: [
    {
      id: 'role-reviewer',
      role: 'reviewer',
      display_label: 'Revisión independiente',
      capabilities: ['formal_review'],
      members: [],
    },
    {
      id: 'role-implementer',
      role: 'implementer',
      display_label: 'Implementación',
      capabilities: [],
      members: [],
    },
    {
      id: 'role-specialist',
      role: 'specialist',
      display_label: 'Especialista',
      capabilities: [],
      members: [],
    },
  ],
} as unknown as TaskCoordinationOverviewResponse

function escalatedRequest(): TaskQualityRequestResponse {
  return {
    id: 'quality-3',
    task_id: 'task-1',
    source_kind: 'review_report',
    source_id: 'artifact-1',
    category: 'delivery_correction',
    target_kind: 'implementation',
    target_task_role_id: 'role-implementer',
    target_actor: null,
    summary: 'Correct the exact finding',
    findings: ['Missing evidence'],
    resolution_condition: 'An exact ReviewReport passes',
    state: 'requires_human',
    handoff_id: null,
    work_unit_id: null,
    coordination_request_id: null,
    budget_round: 3,
    version: 1,
    created_at: '',
    updated_at: '',
    resolved_at: null,
    resolution_kind: null,
    resolution_ref: null,
    resolution_digest: null,
    quality_route: {
      id: 'review_line:line-1',
      round: 3,
      limit: 4,
      source_task_role_id: 'role-reviewer',
      escalation: {
        reason_code: 'coordination.quality_route_ping_pong',
        reason: 'pending quality targets alternate between TaskRoles reviewer and implementer',
        cycle_request_ids: ['quality-1', 'quality-2', 'quality-3'],
        cycle_task_role_ids: ['role-reviewer', 'role-implementer', 'role-reviewer'],
      },
    },
  }
}

describe('quality route escalation presentation', () => {
  it('shows the durable route round, escalation reason, and readable role cycle', () => {
    quality.items = [escalatedRequest()]
    render(
      <TaskQualityRequestsPanel
        task={task}
        coordination={coordination}
        agents={[]}
        members={[]}
        daemons={[]}
      />,
    )

    expect(screen.getByText('Route 3 / 4')).toBeTruthy()
    expect(screen.getByText('Forge escalated this quality route to a human')).toBeTruthy()
    expect(
      screen.getByText(
        'pending quality targets alternate between TaskRoles reviewer and implementer',
      ),
    ).toBeTruthy()
    expect(
      screen.getByText('Pending role chain: Revisión independiente → Implementación → Revisión independiente'),
    ).toBeTruthy()
  })
})
