import type { ReactNode } from 'react'
import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  ProjectPlanBatchResponse,
  ProjectPlanGraphResponse,
  ProjectPlanNodeResponse,
  ProjectPlanProposalResponse,
} from '@/types/generated'
import i18n from '@/lib/i18n'
import { ApiError } from '@/api/client'
import { useAuthStore } from '@/stores/auth'
import { ProjectPlanPage } from './ProjectPlanPage'

type LinkProps = {
  to: string
  params?: Record<string, string>
  className?: string
  children: ReactNode
}

const mocks = vi.hoisted(() => ({
  useProjectPlanQuery: vi.fn(),
  useProjectPlanMutations: vi.fn(),
  useProjectQuery: vi.fn(),
  useMembersQuery: vi.fn(),
}))

vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, params, className, children }: LinkProps) => {
    const href = params
      ? Object.entries(params).reduce((path, [key, value]) => path.replace(`$${key}`, value), to)
      : to
    return (
      <a href={href} className={className}>
        {children}
      </a>
    )
  },
}))

vi.mock('./hooks', () => ({
  useProjectPlanQuery: mocks.useProjectPlanQuery,
  useProjectPlanMutations: mocks.useProjectPlanMutations,
}))

vi.mock('@/api/hooks', () => ({
  useProjectQuery: mocks.useProjectQuery,
  useMembersQuery: mocks.useMembersQuery,
}))

const emptyForecast = {
  critical_path_task_ids: [],
  min_minutes: null,
  max_minutes: null,
  incomplete_estimates: true,
  basis: 'task_estimates',
}

function planNode(
  key: string,
  title: string,
  options: Partial<ProjectPlanNodeResponse> = {},
): ProjectPlanNodeResponse {
  return {
    key,
    task_id: `task-${key}`,
    title,
    lifecycle_state: 'ready',
    dispatch_state: 'idle',
    readiness: 'ready',
    blockers: [],
    unsatisfied_task_ids: [],
    parallel_level: 0,
    running_executions: 0,
    estimate_min_minutes: 10,
    estimate_max_minutes: 20,
    phase: 'build',
    source_ids: [],
    ...options,
  }
}

function batch(state: string): ProjectPlanBatchResponse {
  return {
    id: `batch-${state}`,
    project_id: 'project-1',
    graph_version: 4,
    version: 7,
    state,
    task_ids: ['task-a', 'task-b'],
    max_parallel_tasks: 2,
    authorized_by: 'owner-1',
    created_at: '2026-11-11T10:00:00Z',
    updated_at: '2026-11-11T10:00:00Z',
  }
}

function proposal(version = 4): ProjectPlanProposalResponse {
  return {
    id: 'proposal-1',
    project_id: 'project-1',
    base_version: version,
    document: {
      nodes: [
        {
          key: 'a',
          task_id: 'task-existing',
          title: 'Reuse existing Task',
          description: 'Keep its current identity.',
          acceptance_criteria: ['Existing evidence stays linked'],
          phase: 'build',
          estimate_min_minutes: 10,
          estimate_max_minutes: 20,
          source_ids: ['source-1'],
        },
        {
          key: 'b',
          task_id: null,
          title: 'Create follow-up Task',
          description: null,
          acceptance_criteria: [],
          phase: 'release',
          estimate_min_minutes: null,
          estimate_max_minutes: null,
          source_ids: [],
        },
      ],
      edges: [
        { predecessor_key: 'a', successor_key: 'b', inferred: true, source_ids: ['source-1'] },
      ],
      sources: [
        {
          id: 'source-1',
          label: 'Project brief',
          kind: 'document',
          reference: 'brief.md',
          revision: 'r3',
          excerpt: 'Deliver the project in two phases.',
        },
      ],
    },
    digest: 'proposal-digest-0123456789abcdef',
    state: 'proposed',
    applied_version: null,
    created_at: '2026-11-11T10:00:00Z',
  }
}

function graph(overrides: Partial<ProjectPlanGraphResponse> = {}): ProjectPlanGraphResponse {
  return {
    project_id: 'project-1',
    version: 4,
    nodes: [],
    edges: [],
    sources: [],
    proposals: [],
    batches: [],
    forecast: emptyForecast,
    ...overrides,
  }
}

function setupPage(value: ProjectPlanGraphResponse, role = 'owner', isAdmin = false) {
  const refetch = vi.fn()
  const apply = { mutate: vi.fn(), isPending: false }
  const authorize = { mutate: vi.fn(), isPending: false }
  const updateBatch = { mutate: vi.fn(), isPending: false }
  mocks.useProjectPlanQuery.mockReturnValue({
    data: value,
    isLoading: false,
    isFetching: false,
    error: null,
    refetch,
  })
  mocks.useProjectPlanMutations.mockReturnValue({ apply, authorize, updateBatch })
  mocks.useProjectQuery.mockReturnValue({
    data: { id: 'project-1', owner_id: 'owner-1' },
  })
  mocks.useMembersQuery.mockReturnValue({
    data:
      role === 'owner'
        ? [
            {
              id: 'member-1',
              user_id: 'owner-1',
              role: 'owner',
              email: 'owner@example.test',
            },
          ]
        : [
            {
              id: 'member-owner',
              user_id: 'owner-1',
              role: 'owner',
              email: 'owner@example.test',
            },
            {
              id: 'member-1',
              user_id: 'member-1',
              role,
              email: 'member@example.test',
            },
          ],
  })
  useAuthStore.setState({
    user: {
      id: role === 'owner' ? 'owner-1' : 'member-1',
      email: 'owner@example.test',
      display_name: 'Project owner',
      is_admin: isAdmin,
      created_at: '2026-11-01T00:00:00Z',
    },
  })
  const view = render(<ProjectPlanPage projectId="project-1" />)
  return { ...view, apply, authorize, updateBatch, refetch }
}

describe('ProjectPlanPage', () => {
  beforeEach(async () => {
    await i18n.changeLanguage('en')
    mocks.useProjectPlanQuery.mockReset()
    mocks.useProjectPlanMutations.mockReset()
    mocks.useProjectQuery.mockReset()
    mocks.useMembersQuery.mockReset()
  })

  afterEach(() => {
    useAuthStore.setState({ user: null })
  })

  it('renders a diamond DAG, distinguishes potential and running work, and filters by phase', () => {
    const nodes = [
      planNode('a', 'Task A', { parallel_level: 0, phase: 'build' }),
      planNode('b', 'Task B', { parallel_level: 1, phase: 'build', running_executions: 1 }),
      planNode('c', 'Task C', { parallel_level: 1, phase: 'build', running_executions: 2 }),
      planNode('d', 'Task D', { parallel_level: 2, phase: 'release' }),
    ]
    const edges = [
      { predecessor_key: 'a', successor_key: 'b', inferred: false, source_ids: [] },
      { predecessor_key: 'a', successor_key: 'c', inferred: true, source_ids: [] },
      { predecessor_key: 'b', successor_key: 'd', inferred: false, source_ids: [] },
      { predecessor_key: 'c', successor_key: 'd', inferred: false, source_ids: [] },
    ]
    setupPage(
      graph({
        nodes,
        edges,
        forecast: {
          critical_path_task_ids: ['task-a', 'task-b', 'task-d'],
          min_minutes: 30,
          max_minutes: 60,
          incomplete_estimates: false,
          basis: 'task_estimates',
        },
      }),
    )

    const graphRegion = screen.getByRole('region', { name: 'Project Task dependency graph' })
    expect(within(graphRegion).getAllByRole('article')).toHaveLength(4)
    expect(document.querySelectorAll('path[data-project-plan-edge]')).toHaveLength(4)
    expect(screen.getByText('Potential parallel width').parentElement?.textContent).toContain('2')
    expect(
      screen.getByText('Tasks with running Executions · Executions').parentElement?.textContent,
    ).toContain('2 · 3')
    expect(screen.getByText('30–60 min')).toBeTruthy()
    expect(screen.getByText(/Critical path Tasks:/)).toBeTruthy()

    fireEvent.change(screen.getByRole('combobox', { name: 'Filter by phase' }), {
      target: { value: 'build' },
    })
    expect(within(graphRegion).getAllByRole('article')).toHaveLength(3)
    expect(within(graphRegion).queryByRole('link', { name: 'Task D' })).toBeNull()
    expect(
      within(screen.getByRole('table', { name: 'Task dependency links' })).getAllByRole('row'),
    ).toHaveLength(3)
  })

  it('keeps critical path duration unknown when any estimate is missing', () => {
    setupPage(
      graph({
        nodes: [
          planNode('a', 'Task A', { estimate_min_minutes: null, estimate_max_minutes: null }),
        ],
        forecast: emptyForecast,
      }),
    )

    expect(screen.getByText('Critical path duration').parentElement?.textContent).toContain(
      'Unknown',
    )
    expect(
      screen.getByText('Unknown if any Task estimate is missing or invalid; unknown is not zero.'),
    ).toBeTruthy()
    expect(screen.queryByText('Critical path Tasks')).toBeNull()
  })

  it('shows the optional empty state and a path to manual Tasks', () => {
    setupPage(graph())

    expect(screen.getByText('The project dependency plan is optional')).toBeTruthy()
    expect(
      screen.getByRole('heading', { name: 'No project Tasks are in this graph yet' }),
    ).toBeTruthy()
    expect(screen.getByRole('link', { name: 'Open Tasks' }).getAttribute('href')).toBe(
      '/projects/project-1/tasks',
    )
  })

  it('shows proposal mapping, inferred edges, and source revisions before applying', () => {
    const { apply } = setupPage(graph({ proposals: [proposal()] }))

    fireEvent.click(screen.getByText('2 proposed Tasks · Proposed'))
    expect(screen.getByText('Maps to Task task-existing')).toBeTruthy()
    expect(screen.getByText('Creates a new Task')).toBeTruthy()
    expect(screen.getAllByText('Inferred').length).toBeGreaterThan(0)
    expect(screen.getByText('revision r3')).toBeTruthy()
    expect(screen.getByText('Deliver the project in two phases.')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Apply proposal' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Apply proposal' }))
    expect(apply.mutate).toHaveBeenCalledWith(
      {
        proposalId: 'proposal-1',
        body: { expected_version: 4, idempotency_key: expect.any(String) },
      },
      expect.objectContaining({ onSuccess: expect.any(Function), onError: expect.any(Function) }),
    )
  })

  it('retries a partially applied proposal with its original graph version and idempotency key', () => {
    const page = setupPage(graph({ proposals: [proposal()] }))

    fireEvent.click(screen.getByText('2 proposed Tasks · Proposed'))
    fireEvent.click(screen.getByRole('button', { name: 'Apply proposal' }))
    const firstRequest = page.apply.mutate.mock.calls[0]?.[0] as {
      body: { expected_version: number; idempotency_key: string }
    }
    const firstOptions = page.apply.mutate.mock.calls[0]?.[1] as {
      onError: (error: unknown) => void
    }
    firstOptions.onError(
      new ApiError(
        JSON.stringify({ code: 'project_plan_apply_incomplete', message: 'Import can be resumed' }),
        409,
      ),
    )

    mocks.useProjectPlanQuery.mockReturnValue({
      data: graph({ version: 5, proposals: [proposal(4)] }),
      isLoading: false,
      isFetching: false,
      error: null,
      refetch: page.refetch,
    })
    page.rerender(<ProjectPlanPage projectId="project-1" />)
    expect(
      screen.getByText(
        'A previous apply left a staged Task that is blocked from work. Retry reuses the original graph version and idempotency key; it resumes only if the graph is still at that version. If another change advanced the graph, refresh and review the proposal after the version conflict.',
      ),
    ).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Retry partial apply' }))
    const secondRequest = page.apply.mutate.mock.calls[1]?.[0] as {
      proposalId: string
      body: { expected_version: number; idempotency_key: string }
    }
    expect(secondRequest).toEqual({
      proposalId: 'proposal-1',
      body: {
        expected_version: firstRequest.body.expected_version,
        idempotency_key: firstRequest.body.idempotency_key,
      },
    })
    expect(secondRequest.body).toEqual({ expected_version: 4, idempotency_key: expect.any(String) })

    const retryOptions = page.apply.mutate.mock.calls[1]?.[1] as {
      onError: (error: unknown) => void
    }
    act(() => {
      retryOptions.onError(
        new ApiError(
          JSON.stringify({
            code: 'version_conflict',
            message: 'Graph changed since the partial apply',
          }),
          409,
        ),
      )
    })
    expect(
      screen.getByText(
        'This proposal is based on an older graph version. Refresh or ask Copilot for an updated proposal before applying it.',
      ),
    ).toBeTruthy()
    expect(
      (screen.getByRole('button', { name: 'Apply proposal' }) as HTMLButtonElement).disabled,
    ).toBe(true)
  })

  it('keeps the apply key stable when the page is reopened after a partial import', () => {
    const firstPage = setupPage(graph({ proposals: [proposal(4)] }))
    fireEvent.click(screen.getByText('2 proposed Tasks · Proposed'))
    fireEvent.click(screen.getByRole('button', { name: 'Apply proposal' }))
    const firstRequest = firstPage.apply.mutate.mock.calls[0]?.[0] as {
      body: { expected_version: number; idempotency_key: string }
    }
    firstPage.unmount()

    const reopenedPage = setupPage(graph({ proposals: [proposal(4)] }))
    fireEvent.click(screen.getByText('2 proposed Tasks · Proposed'))
    fireEvent.click(screen.getByRole('button', { name: 'Apply proposal' }))
    const replayRequest = reopenedPage.apply.mutate.mock.calls[0]?.[0] as {
      body: { expected_version: number; idempotency_key: string }
    }
    expect(replayRequest.body).toEqual(firstRequest.body)
    expect(replayRequest.body).toEqual({
      expected_version: 4,
      idempotency_key: 'project-plan-apply:proposal-1',
    })
  })

  it('authorizes the explicitly selected phase slice with a separate Task concurrency cap', () => {
    const { authorize } = setupPage(
      graph({
        nodes: [
          planNode('a', 'Task A', { phase: 'build' }),
          planNode('b', 'Task B', { phase: 'release', parallel_level: 1 }),
        ],
      }),
    )

    fireEvent.change(screen.getByRole('combobox', { name: 'Filter by phase' }), {
      target: { value: 'build' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Select or clear visible Tasks' }))
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Maximum parallel Tasks' }), {
      target: { value: '3' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Authorize 1 selected' }))

    expect(authorize.mutate).toHaveBeenCalledWith(
      {
        expected_version: 4,
        task_ids: ['task-a'],
        max_parallel_tasks: 3,
        idempotency_key: expect.any(String),
      },
      expect.objectContaining({ onSuccess: expect.any(Function), onError: expect.any(Function) }),
    )
  })

  it('authorizes dependency-blocked backlog Tasks with their prerequisite in one batch', () => {
    const { authorize } = setupPage(
      graph({
        nodes: [
          planNode('a', 'Task A', { lifecycle_state: 'backlog', phase: 'build' }),
          planNode('b', 'Task B', {
            lifecycle_state: 'backlog',
            readiness: 'blocked',
            blockers: ['dependency:task-a'],
            unsatisfied_task_ids: ['task-a'],
            parallel_level: 1,
            phase: 'build',
          }),
        ],
        edges: [{ predecessor_key: 'a', successor_key: 'b', inferred: false, source_ids: [] }],
      }),
    )

    expect(
      screen.getByText(
        'Backlog, ready, and lifecycle-blocked Tasks can be authorized together, including Tasks blocked by prerequisites; the scheduler waits for accepted handoffs. Active, merging, ready-to-merge, done, and cancelled Tasks cannot be replanned here.',
      ),
    ).toBeTruthy()
    expect(
      (
        screen.getByRole('checkbox', {
          name: 'Select Task Task B for batch authorization',
        }) as HTMLInputElement
      ).disabled,
    ).toBe(false)
    fireEvent.click(screen.getByRole('button', { name: 'Select or clear visible Tasks' }))
    fireEvent.click(screen.getByRole('button', { name: 'Authorize 2 selected' }))

    expect(authorize.mutate).toHaveBeenCalledWith(
      {
        expected_version: 4,
        task_ids: ['task-a', 'task-b'],
        max_parallel_tasks: 1,
        idempotency_key: expect.any(String),
      },
      expect.objectContaining({ onSuccess: expect.any(Function), onError: expect.any(Function) }),
    )
  })

  it('does not offer already active, merging, ready-to-merge, or terminal Tasks for re-planning', () => {
    setupPage(
      graph({
        nodes: [
          planNode('blocked', 'Lifecycle blocked', {
            lifecycle_state: 'blocked',
            readiness: 'blocked',
            blockers: ['lifecycle:blocked'],
          }),
          planNode('active', 'Active Task', { lifecycle_state: 'active' }),
          planNode('ready-merge', 'Ready-to-merge Task', { lifecycle_state: 'ready_to_merge' }),
          planNode('merge', 'Merging Task', { lifecycle_state: 'merging' }),
          planNode('done', 'Done Task', { lifecycle_state: 'done' }),
          planNode('cancelled', 'Cancelled Task', { lifecycle_state: 'cancelled' }),
        ],
      }),
    )

    expect(
      (
        screen.getByRole('checkbox', {
          name: 'Select Task Lifecycle blocked for batch authorization',
        }) as HTMLInputElement
      ).disabled,
    ).toBe(false)
    for (const title of [
      'Active Task',
      'Ready-to-merge Task',
      'Merging Task',
      'Done Task',
      'Cancelled Task',
    ]) {
      expect(
        (
          screen.getByRole('checkbox', {
            name: `Select Task ${title} for batch authorization`,
          }) as HTMLInputElement
        ).disabled,
      ).toBe(true)
    }
    fireEvent.click(screen.getByRole('button', { name: 'Select or clear visible Tasks' }))
    expect(screen.getByText('1 selected overall')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Authorize 1 selected' })).toBeTruthy()
  })

  it('offers pause and revoke for a running batch and resume and revoke for a paused batch', () => {
    const running = setupPage(graph({ batches: [batch('running')] }))
    expect(screen.getByRole('button', { name: 'Pause batch' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Revoke authority' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Pause batch' }))
    expect(running.updateBatch.mutate).toHaveBeenCalledWith(
      {
        batchId: 'batch-running',
        body: { expected_version: 7, action: 'pause', idempotency_key: expect.any(String) },
      },
      expect.objectContaining({ onSuccess: expect.any(Function), onError: expect.any(Function) }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Revoke authority' }))
    expect(running.updateBatch.mutate).toHaveBeenLastCalledWith(
      {
        batchId: 'batch-running',
        body: { expected_version: 7, action: 'revoke', idempotency_key: expect.any(String) },
      },
      expect.objectContaining({ onSuccess: expect.any(Function), onError: expect.any(Function) }),
    )

    running.unmount()
    const paused = setupPage(graph({ batches: [batch('paused')] }))
    expect(screen.getByRole('button', { name: 'Resume batch' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Revoke authority' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Pause batch' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Resume batch' }))
    expect(paused.updateBatch.mutate).toHaveBeenCalledWith(
      {
        batchId: 'batch-paused',
        body: { expected_version: 7, action: 'resume', idempotency_key: expect.any(String) },
      },
      expect.objectContaining({ onSuccess: expect.any(Function), onError: expect.any(Function) }),
    )
    paused.unmount()
  })

  it('allows Project writers to apply proposals but reserves batch authorization for admins', () => {
    setupPage(graph({ nodes: [planNode('a', 'Task A')], proposals: [proposal()] }), 'member')

    fireEvent.click(screen.getByText('2 proposed Tasks · Proposed'))
    expect(screen.getByRole('button', { name: 'Apply proposal' })).toHaveProperty('disabled', false)
    expect(screen.queryByRole('button', { name: /Authorize/ })).toBeNull()
    expect(
      screen.getByRole('checkbox', { name: 'Select Task Task A for batch authorization' }),
    ).toHaveProperty('disabled', true)
  })

  it('does not treat global admin status as Project write or batch authority', () => {
    setupPage(graph({ nodes: [planNode('a', 'Task A')], proposals: [proposal()] }), 'viewer', true)

    fireEvent.click(screen.getByText('2 proposed Tasks · Proposed'))
    expect(screen.getByRole('button', { name: 'Apply proposal' })).toHaveProperty('disabled', true)
    expect(screen.queryByRole('button', { name: /Authorize/ })).toBeNull()
    expect(
      screen.getByRole('checkbox', { name: 'Select Task Task A for batch authorization' }),
    ).toHaveProperty('disabled', true)
  })
})
