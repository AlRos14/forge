import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { TaskCreateDialog } from '@/components/task-create-dialog'
import type { Execution, Task, WorkUnitResponse } from '@/types/generated'
import { TaskWorkManagement } from './task-work-management'

const { createTaskMutation } = vi.hoisted(() => ({ createTaskMutation: vi.fn() }))

vi.mock('@/components/ui/markdown-editor', () => ({
  MarkdownEditor: ({
    ariaLabel,
    value,
    onChange,
  }: {
    ariaLabel?: string
    value: string
    onChange: (value: string) => void
  }) => (
    <textarea
      aria-label={ariaLabel}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
  MarkdownView: ({ content }: { content: string }) => <div>{content}</div>,
}))

vi.mock('@/api/hooks', () => ({
  useAgentsInfiniteQuery: () => ({
    data: { pages: [{ items: [] }] },
    isError: false,
    isLoading: false,
  }),
  useAgentsQuery: () => ({ data: { items: [] }, isError: false, isLoading: false }),
  useCreateTask: () => ({ mutate: createTaskMutation, isPending: false }),
  useDaemonsQuery: () => ({ data: { items: [] }, isError: false, isLoading: false }),
  useMembersQuery: () => ({ data: [], isError: false, isLoading: false }),
  useProjectQuery: () => ({
    data: { owner_id: 'owner-1', primary_repo_id: 'repo-ui' },
    isError: false,
    isLoading: false,
  }),
  useProjectTaskRoleDefaultsQuery: () => ({
    data: {
      roles: [
        {
          role: 'implementer',
          configuration: { display_label: 'Implementer', capabilities: [] },
          members: [],
        },
      ],
    },
    isError: false,
    isLoading: false,
  }),
  useReposQuery: () => ({
    data: {
      items: [
        { id: 'repo-ui', name: 'Web UI' },
        { id: 'repo-api', name: 'API' },
      ],
    },
    isError: false,
    isLoading: false,
  }),
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
    repository_ids: [],
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
    integrations: [],
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
  afterEach(() => {
    vi.restoreAllMocks()
    createTaskMutation.mockReset()
  })

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
        repository_results: [],
      },
    }
    const fetchMock = vi.spyOn(window, 'fetch').mockImplementation(async (input, init) => {
      const path = new URL(input.toString()).pathname
      if (path === '/api/v1/task-role-capabilities') {
        return new Response(
          JSON.stringify({
            schema_version: 1,
            capabilities: ['formal_review', 'orchestration', 'request_rework'],
            templates: [],
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        )
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
      repo_id: null,
      repositories: [],
      repository_set_revision: 0,
      repository_set_digest: null,
      repository_merge_batches: [],
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
    const createRequest = fetchMock.mock.calls.find(
      ([input, init]) =>
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

  it('multirepo_task_create_and_work_unit_repository_selectors', async () => {
    const created = sourceUnit()
    const fetchMock = vi.spyOn(window, 'fetch').mockImplementation(async (input, init) => {
      const path = new URL(input.toString()).pathname
      if (path === '/api/v1/projects/project-1/repos') {
        return new Response(
          JSON.stringify({
            items: [
              { id: 'repo-ui', name: 'Web UI' },
              { id: 'repo-api', name: 'API' },
            ],
            next_cursor: null,
            has_more: false,
            total_count: 2,
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        )
      }
      if (path === '/api/v1/task-role-capabilities') {
        return new Response(
          JSON.stringify({
            schema_version: 1,
            capabilities: ['formal_review', 'orchestration', 'request_rework'],
            templates: [],
          }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        )
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
    const taskCreate = render(
      <TaskCreateDialog open projectId="project-1" onOpenChange={vi.fn()} />,
    )
    fireEvent.change(screen.getByLabelText('Task'), {
      target: { value: '# Update Web UI\nImplement the new interface.' },
    })
    fireEvent.change(await screen.findByLabelText('Repository role for API'), {
      target: { value: 'affected' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Create Task' }))

    await waitFor(() => expect(createTaskMutation).toHaveBeenCalled())
    expect(createTaskMutation.mock.calls[0]?.[0]).toMatchObject({
      repositories: [
        { repo_id: 'repo-ui', role: 'primary' },
        { repo_id: 'repo-api', role: 'affected' },
      ],
    })
    taskCreate.unmount()

    const task = {
      id: 'task-1',
      project_id: 'project-1',
      repo_id: 'repo-ui',
      repository_set_revision: 3,
      repository_set_digest: null,
      repository_merge_batches: [],
      repositories: [
        { repo_id: 'repo-ui', role: 'primary', position: 0 },
        { repo_id: 'repo-api', role: 'affected', position: 1 },
        { repo_id: 'repo-contracts', role: 'context', position: 2 },
      ],
      task_roles: [role('role-implementer', 'implementer')],
    } as unknown as Task
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })

    render(
      <QueryClientProvider client={queryClient}>
        <TaskWorkManagement task={task} workUnits={[]} executions={[]} onChanged={vi.fn()} />
      </QueryClientProvider>,
    )

    const form = screen.getByRole('button', { name: 'Create WorkUnit' }).closest('form')
    if (!form) throw new Error('WorkUnit create form is missing')
    fireEvent.change(within(form).getByLabelText('Title'), {
      target: { value: 'Update API' },
    })
    fireEvent.change(within(form).getByLabelText('Scope'), {
      target: { value: 'Update the API contract.' },
    })
    fireEvent.click(screen.getByRole('checkbox', { name: 'API · affected' }))
    fireEvent.click(screen.getByRole('button', { name: 'Create WorkUnit' }))

    await waitFor(() => expect(fetchMock).toHaveBeenCalled())
    const workUnitRequest = fetchMock.mock.calls.find(
      ([input, init]) =>
        new URL(input.toString()).pathname === '/api/v1/tasks/task-1/work-units' &&
        init?.method === 'POST',
    )
    expect(workUnitRequest).toBeDefined()
    expect(JSON.parse(String(workUnitRequest?.[1]?.body))).toMatchObject({
      repository_ids: ['repo-api'],
    })
    queryClient.clear()
  })

  it('multirepo_lifecycle_partial_batches_visible_and_retryable', async () => {
    const unit = {
      ...sourceUnit(),
      requires_integration: true,
      integrations: [
        {
          integration_id: 'integration-primary',
          parent_integration_id: 'parent-integration',
          task_id: 'task-1',
          work_unit_id: 'unit-implementation',
          execution_id: 'execution-1',
          repository_set_revision: 2,
          repo_id: 'repo-primary',
          source_workspace_id: 'source-primary',
          source_branch: 'agent/task-1',
          source_sha: 'a'.repeat(40),
          target_workspace_id: 'target-primary',
          target_branch: 'main',
          target_before_sha: 'b'.repeat(40),
          target_after_sha: null,
          outcome: 'conflict' as const,
          version: 1,
          started_at: '',
          finished_at: '',
        },
        {
          integration_id: 'integration-affected',
          parent_integration_id: 'parent-integration',
          task_id: 'task-1',
          work_unit_id: 'unit-implementation',
          execution_id: 'execution-1',
          repository_set_revision: 2,
          repo_id: 'repo-affected',
          source_workspace_id: 'source-affected',
          source_branch: 'agent/task-1',
          source_sha: 'c'.repeat(40),
          target_workspace_id: 'target-affected',
          target_branch: 'main',
          target_before_sha: 'd'.repeat(40),
          target_after_sha: 'e'.repeat(40),
          outcome: 'success' as const,
          version: 1,
          started_at: '',
          finished_at: '',
        },
      ],
    }
    const partialResponse = {
      id: 'parent-integration',
      repositories: [
        { integration_id: 'integration-primary', outcome: 'conflict', repo_id: 'repo-primary' },
        { integration_id: 'integration-affected', outcome: 'success', repo_id: 'repo-affected' },
      ],
    }
    const fetchMock = vi.spyOn(window, 'fetch').mockImplementation(async (input) => {
      const path = new URL(input.toString()).pathname
      if (path.endsWith('/integrations')) {
        return new Response(JSON.stringify(partialResponse), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        })
      }
      if (path.endsWith('/dependencies')) return new Response('[]', { status: 200 })
      if (path === '/api/v1/task-role-capabilities') {
        return new Response(
          JSON.stringify({ schema_version: 1, capabilities: [], templates: [] }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        )
      }
      return new Response('[]', { status: 200 })
    })
    const task = {
      id: 'task-1',
      project_id: 'project-1',
      repo_id: null,
      repositories: [],
      repository_set_revision: 0,
      repository_set_digest: null,
      repository_merge_batches: [],
      task_roles: [role('role-implementer', 'implementer')],
    } as unknown as Task
    const execution = {
      id: 'execution-1',
      status: 'completed',
      role: 'implementer',
      actor_ref: null,
    } as unknown as Execution
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })

    render(
      <QueryClientProvider client={queryClient}>
        <TaskWorkManagement
          task={task}
          workUnits={[unit]}
          executions={[execution]}
          onChanged={vi.fn().mockResolvedValue(undefined)}
        />
      </QueryClientProvider>,
    )

    expect(screen.getByText(/repo-primary · conflict/)).toBeTruthy()
    expect(screen.getByText(/repo-affected · success/)).toBeTruthy()
    const retry = screen.getByRole('button', {
      name: 'Retry pending repository integrations',
    })
    fireEvent.click(retry)
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.filter(([input]) =>
          new URL(input.toString()).pathname.endsWith('/integrations'),
        ),
      ).toHaveLength(1),
    )
    fireEvent.click(retry)
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.filter(([input]) =>
          new URL(input.toString()).pathname.endsWith('/integrations'),
        ),
      ).toHaveLength(2),
    )
    const requestBodies = fetchMock.mock.calls
      .filter(([input]) => new URL(input.toString()).pathname.endsWith('/integrations'))
      .map(
        ([, init]) =>
          JSON.parse(String(init?.body)) as { execution_id: string; idempotency_key: string },
      )
    expect(requestBodies).toEqual([
      {
        execution_id: 'execution-1',
        idempotency_key: 'work-unit-integration:unit-implementation:execution-1',
      },
      {
        execution_id: 'execution-1',
        idempotency_key: 'work-unit-integration:unit-implementation:execution-1',
      },
    ])
    queryClient.clear()
  })
})
