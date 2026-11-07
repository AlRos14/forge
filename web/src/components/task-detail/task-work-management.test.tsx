import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Task, WorkUnitResponse } from '@/types/generated'
import { TaskWorkManagement } from './task-work-management'

vi.mock('@/api/hooks', () => ({
  useAgentsQuery: () => ({ data: { items: [] }, isError: false, isLoading: false }),
  useMembersQuery: () => ({ data: [], isError: false, isLoading: false }),
  useProjectQuery: () => ({ data: { owner_id: 'owner-1' }, isError: false, isLoading: false }),
  useTaskGatesQuery: () => ({ data: [], isLoading: false }),
  useTaskReviewLinesQuery: () => ({ data: [], isLoading: false }),
  useWorkUnitArtifactPackageQuery: () => ({
    data: { work_unit_id: 'unit-implementation', inputs: [], manifests: [] },
    isError: false,
  }),
}))

function role(id: string, name: string) {
  return {
    id,
    task_id: 'task-1',
    role: name,
    coordination_mode: 'collaborative',
    policy: {},
    configuration: {
      schema_version: 1,
      display_label: null,
      capabilities: name === 'reviewer' ? ['formal_review'] : [],
      rework: null,
    },
    version: 1,
    members: [],
    created_at: '',
    updated_at: '',
  }
}

function sourceUnit(): WorkUnitResponse {
  return {
    id: 'unit-implementation',
    task_id: 'task-1',
    parent_work_unit_id: null,
    title: 'Implement feature',
    scope: 'Implement the requested feature.',
    execution_instructions: null,
    status: 'open',
    role: 'implementer',
    assigned_actor: { kind: 'agent', id: 'agent-1' },
    requires_integration: false,
    provenance: null,
    review_approval_gate_id: null,
    review_approval_line_id: null,
    created_by: { kind: 'human', id: 'user-1' },
    version: 2,
    created_at: '',
    updated_at: '',
    review_subject: null,
    readiness: {
      runnable: false,
      ready_for_allocation: false,
      active_execution_ids: [],
      unsatisfied_dependency_ids: [],
      awaiting_integration: false,
      awaiting_acceptance: false,
      awaiting_review_approval: false,
    },
  }
}

describe('TaskWorkManagement reviewer setup', () => {
  afterEach(() => vi.restoreAllMocks())

  it('creates a reviewer WorkUnit pinned to the selected implementation WorkUnit', async () => {
    const created = {
      ...sourceUnit(),
      id: 'unit-review',
      title: 'Review implementation',
      role: 'reviewer',
      review_subject: {
        id: 'unit-review',
        review_work_unit_id: 'unit-review',
        reviewed_work_unit_id: 'unit-implementation',
        reviewed_work_unit_version: null,
        result_execution_id: null,
        result_artifact_id: null,
        result_commit_sha: null,
        result_workspace_id: null,
        workspace_snapshot_digest: null,
      },
    }
    const fetchMock = vi.spyOn(window, 'fetch').mockImplementation(async (input, init) => {
      const path = new URL(input.toString()).pathname
      if (path === '/api/v1/task-role-capabilities') {
        return new Response(JSON.stringify({
          schema_version: 1,
          capabilities: ['formal_review', 'orchestration', 'request_rework'],
          templates: [],
        }), { status: 200, headers: { 'content-type': 'application/json' } })
      }
      if (path.endsWith('/dependencies')) return new Response('[]', { status: 200 })
      if (path === '/api/v1/tasks/task-1/work-units' && init?.method === 'POST') {
        return new Response(JSON.stringify(created), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        })
      }
      return new Response('[]', { status: 200 })
    })
    const onChanged = vi.fn().mockResolvedValue(undefined)
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    const task = {
      id: 'task-1',
      project_id: 'project-1',
      task_roles: [role('role-implementer', 'implementer'), role('role-reviewer', 'reviewer')],
    } as unknown as Task

    render(
      <QueryClientProvider client={queryClient}>
        <TaskWorkManagement
          task={task}
          workUnits={[sourceUnit()]}
          executions={[]}
          onChanged={onChanged}
        />
      </QueryClientProvider>,
    )

    const createButton = screen.getByRole('button', { name: 'Create WorkUnit' })
    const form = createButton.closest('form')
    if (!form) throw new Error('WorkUnit create form is missing')
    fireEvent.change(within(form).getByLabelText('Title'), {
      target: { value: 'Review implementation' },
    })
    fireEvent.change(within(form).getByLabelText('TaskRole'), {
      target: { value: 'reviewer' },
    })
    fireEvent.change(within(form).getByLabelText('Scope'), {
      target: { value: 'Review the exact implementation result.' },
    })
    fireEvent.change(within(form).getByLabelText(/Exact Artifact inputs/), {
      target: { value: 'artifact-a\nartifact-b' },
    })
    fireEvent.change(screen.getByLabelText('Exact WorkUnit to review'), {
      target: { value: 'unit-implementation' },
    })
    fireEvent.click(createButton)

    await waitFor(() => expect(fetchMock).toHaveBeenCalled())
    const createRequest = fetchMock.mock.calls.find(([input, init]) =>
      new URL(input.toString()).pathname === '/api/v1/tasks/task-1/work-units' &&
      init?.method === 'POST',
    )
    expect(createRequest).toBeDefined()
    expect(JSON.parse(String(createRequest?.[1]?.body))).toMatchObject({
      role: 'reviewer',
      review_subject: { reviewed_work_unit_id: 'unit-implementation' },
      artifact_inputs: ['artifact-a', 'artifact-b'],
    })
    expect(onChanged).toHaveBeenCalledTimes(1)
    queryClient.clear()
  })
})
