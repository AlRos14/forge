import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { ReactNode } from 'react'
import type { ReviewExecutionResponse } from '@/types/generated'

const pageMocks = vi.hoisted(() => ({
  launch: vi.fn(),
}))

vi.mock('@/api/hooks', async () => {
  const React = await import('react')
  const task = {
    id: 'task-1',
    project_id: 'project-1',
    repo_id: null,
    parent_task_id: null,
    title: 'Task',
    description: null,
    lifecycle: { task_id: 'task-1', state: 'blocked', version: 2 },
    priority: 0,
    board_position: 0,
    subtask_order: null,
    task_roles: [],
    execution_observability: {},
    workspace: null,
    external_issue_number: null,
    external_issue_url: null,
    version: 3,
    created_at: '',
    updated_at: '',
  }
  const staleDispatch = {
    can_launch: false,
    can_pause: false,
    can_resume: false,
    can_cancel: false,
    blockers: [{ code: 'task_blocked', message: 'blocked', work_unit_id: null }],
    dispatch: { state: 'idle', version: 1 },
    work_units: [],
  }
  const readyDispatch = { ...staleDispatch, can_launch: true, blockers: [] }
  const noMutation = { isPending: false, mutateAsync: vi.fn().mockResolvedValue({}) }
  return {
    useProjectQuery: () => ({ data: { owner_id: 'owner-1' } }),
    useMembersQuery: () => ({ data: [], isLoading: false, isError: false }),
    useAgentsInfiniteQuery: () => ({ data: { pages: [] }, isLoading: false, isError: false }),
    useDaemonsQuery: () => ({ data: { items: [] }, isLoading: false, isError: false }),
    useTaskQuery: () => ({ data: task, isLoading: false, isError: false, refetch: vi.fn() }),
    useTaskLifecycleTransitionsQuery: () => ({ data: [], isLoading: false }),
    useExecutionsQuery: () => ({ data: { items: [] }, refetch: vi.fn() }),
    useReviewsQuery: () => ({ data: [], isLoading: false }),
    useTaskReviewLinesQuery: () => ({
      data: [],
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    }),
    useValidationsQuery: () => ({ data: [], isLoading: false }),
    useTaskGatesQuery: () => ({ data: [], isLoading: false }),
    useTaskWorkUnitsQuery: () => ({ data: [], refetch: vi.fn() }),
    useTaskDispatchQuery: () => {
      const [isReady, setIsReady] = React.useState(false)
      return {
        data: isReady ? readyDispatch : staleDispatch,
        refetch: async () => {
          setIsReady(true)
          return { data: readyDispatch }
        },
      }
    },
    useTransitionTaskLifecycle: () => noMutation,
    useUpdateTask: () => noMutation,
    useEvaluateGate: () => noMutation,
    useLaunchTask: () => ({
      isPending: false,
      mutateAsync: pageMocks.launch.mockResolvedValue({
        admitted_execution_ids: [],
        started_execution_ids: [],
      }),
    }),
    usePauseTask: () => noMutation,
    useResumeTask: () => noMutation,
    useCancelTask: () => noMutation,
  }
})

vi.mock('@/components/task-detail/task-work-management', () => ({
  TaskWorkManagement: ({ onChanged }: { onChanged: () => Promise<unknown> }) => (
    <button type="button" onClick={() => void onChanged()}>
      Create prepared WorkUnit
    </button>
  ),
}))

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: ReactNode }) => <a>{children}</a>,
}))

import { TaskDetailPage, ReviewExecutionCard } from './TaskDetailPage'

function makeReview(
  actor_ref: ReviewExecutionResponse['execution']['actor_ref'],
  status: ReviewExecutionResponse['execution']['status'] = 'running',
  report: ReviewExecutionResponse['report'] = null,
): ReviewExecutionResponse {
  return {
    execution: {
      id: 'execution-1',
      role: 'reviewer',
      status,
      actor_ref,
      workspace_id: null,
      before_sha: null,
      after_sha: null,
    } as ReviewExecutionResponse['execution'],
    report,
  }
}

function renderCard(review: ReviewExecutionResponse) {
  return render(<ReviewExecutionCard review={review} />)
}

function expectTechnicalParagraphContaining(content: string) {
  expect(
    screen.getByText(
      (_, element) => element?.tagName === 'P' && element.textContent?.includes(content) === true,
    ),
  ).toBeTruthy()
}

describe('ReviewExecutionCard authorization', () => {
  it('keeps a running Human Execution as history without a manual submit form', () => {
    renderCard(makeReview({ kind: 'human', id: 'user-1' }))

    expect(screen.getByText('Execution execution-1')).toBeTruthy()
    expect(screen.queryByLabelText('Review summary')).toBeNull()
    expect(
      screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' }),
    ).toBeNull()
  })

  it('keeps another Human Execution visible as history without a submit action', () => {
    renderCard(makeReview({ kind: 'human', id: 'user-2' }))

    expect(screen.getByText('Execution execution-1')).toBeTruthy()
    fireEvent.click(screen.getByText('Technical references'))
    expectTechnicalParagraphContaining('Actor human:user-2')
    expectTechnicalParagraphContaining('Base none · head none')
    expect(screen.queryByLabelText('Review summary')).toBeNull()
    expect(
      screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' }),
    ).toBeNull()
  })

  it('never offers Human submission for an Agent Review Execution', () => {
    renderCard(makeReview({ kind: 'agent', id: 'agent-1' }))

    expect(screen.getByText('Execution execution-1')).toBeTruthy()
    fireEvent.click(screen.getByText('Technical references'))
    expectTechnicalParagraphContaining('Actor agent:agent-1')
    expectTechnicalParagraphContaining('Base none · head none')
    expect(
      screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' }),
    ).toBeNull()
  })

  it('keeps a completed ReviewReport visible without a submit action', () => {
    const report = { id: 'report-1', content: 'Review passed.' } as NonNullable<
      ReviewExecutionResponse['report']
    >
    renderCard(makeReview({ kind: 'human', id: 'user-1' }, 'completed', report))

    expect(screen.getByText('Review passed.')).toBeTruthy()
    expect(screen.queryByLabelText('Review summary')).toBeNull()
    expect(
      screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' }),
    ).toBeNull()
  })
})

describe('Task WorkUnit preparation and launch', () => {
  it('refreshes dispatch after WorkUnit setup so ready work can be started', async () => {
    pageMocks.launch.mockClear()
    render(<TaskDetailPage taskId="task-1" initialTab="work" />)

    expect(screen.queryByRole('button', { name: 'Start ready work' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Create prepared WorkUnit' }))
    const launch = await screen.findByRole('button', { name: 'Start ready work' })
    fireEvent.click(launch)

    await waitFor(() => expect(pageMocks.launch).toHaveBeenCalledTimes(1))
  })
})
