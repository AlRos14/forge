import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useCreateTask, useTasksQuery, useTransitionTaskLifecycle } from '@/api/hooks'
import { BoardPage } from '@/features/board/BoardPage'
import type { Task } from '@/types/generated'

vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn() }))
vi.mock('@/api/hooks', () => ({
  useCreateTask: vi.fn(),
  useTasksQuery: vi.fn(),
  useTransitionTaskLifecycle: vi.fn(),
}))

describe('BoardPage TaskLifecycle authority', () => {
  const mutateAsync = vi.fn()
  const task = {
    id: 'task-ready',
    title: 'Lifecycle ready task',
    task_type: 'implementation',
    priority: 2,
    status: 'done',
    lifecycle: { state: 'ready', version: 7 },
  } as unknown as Task

  beforeEach(() => {
    vi.clearAllMocks()
    vi.stubGlobal('crypto', { randomUUID: () => 'exact-transition-request' })
    vi.mocked(useCreateTask).mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    } as unknown as ReturnType<typeof useCreateTask>)
    vi.mocked(useTasksQuery).mockReturnValue({
      data: { pages: [{ items: [task] }] },
      isError: false,
      isLoading: false,
      hasNextPage: false,
      isFetchingNextPage: false,
      refetch: vi.fn(),
      fetchNextPage: vi.fn(),
    } as unknown as ReturnType<typeof useTasksQuery>)
    vi.mocked(useTransitionTaskLifecycle).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as unknown as ReturnType<typeof useTransitionTaskLifecycle>)
  })

  it('renders and transitions from lifecycle state and its exact version', () => {
    render(<BoardPage projectId="project-1" />)

    expect(screen.getByRole('heading', { name: 'ready' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'active' }))

    expect(mutateAsync).toHaveBeenCalledWith({
      taskId: 'task-ready',
      toState: 'active',
      expectedLifecycleVersion: 7,
      idempotencyKey: 'exact-transition-request',
    })
  })
})
