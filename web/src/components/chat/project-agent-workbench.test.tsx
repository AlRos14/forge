import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import { ProjectAgentWorkbench } from './project-agent-workbench'

const updateProject = vi.fn()
const createTask = vi.fn()
const createDocument = vi.fn()
const createDecision = vi.fn()
const createMilestone = vi.fn()

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}))

vi.mock('@/stores/auth', () => ({
  useAuthStore: (selector: (state: { user: Record<string, unknown> }) => unknown) =>
    selector({
      user: {
        id: 'user-1',
        email: 'owner@example.com',
        display_name: 'Owner',
        is_admin: true,
        created_at: '2026-08-14T10:00:00Z',
      },
    }),
}))

vi.mock('@/api/hooks', () => ({
  useProjectQuery: () => ({
    data: { id: 'project-1', name: 'Atlas', version: 4 },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useProjectOverviewQuery: () => ({
    data: {
      projection_state: 'current',
      task_counts: { total: 3, active: 1, blocked: 0 },
    },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useUpdateProject: () => ({ mutateAsync: updateProject, isPending: false }),
  useCreateTask: () => ({ mutateAsync: createTask, isPending: false }),
}))

vi.mock('@/features/project-workbench/hooks', () => ({
  useCreateWorkbenchDocument: () => ({ mutateAsync: createDocument, isPending: false }),
  useCreateWorkbenchDecision: () => ({ mutateAsync: createDecision, isPending: false }),
  useCreateWorkbenchMilestone: () => ({ mutateAsync: createMilestone, isPending: false }),
}))

describe('ProjectAgentWorkbench', () => {
  beforeEach(() => {
    updateProject.mockReset().mockResolvedValue({ id: 'project-1', name: 'Atlas Next', version: 5 })
    createTask.mockReset().mockResolvedValue({ id: 'task-1', title: 'Ship it' })
    createDocument.mockReset().mockResolvedValue({ id: 'document-1', title: 'Delivery brief' })
    createDecision.mockReset().mockResolvedValue({ id: 'decision-1', question: 'Which path?' })
    createMilestone.mockReset().mockResolvedValue({ id: 'milestone-1', canonical_id: 'M1' })
  })

  it('keeps conversation and Project panes mounted behind the compact tabs', () => {
    render(
      <ProjectAgentWorkbench projectId="project-1">
        <div>Canonical conversation</div>
      </ProjectAgentWorkbench>,
    )

    expect(screen.getByRole('tab', { name: 'Conversation' }).getAttribute('aria-selected')).toBe(
      'true',
    )
    fireEvent.click(screen.getByRole('tab', { name: 'Project' }))
    expect(screen.getByRole('tab', { name: 'Project' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByText('Canonical conversation')).toBeTruthy()
    expect(screen.getByLabelText('Project name')).toBeTruthy()
  })

  it('supports arrow-key navigation across compact workspace panes', () => {
    render(
      <ProjectAgentWorkbench projectId="project-1">
        <div>Canonical conversation</div>
      </ProjectAgentWorkbench>,
    )

    const conversation = screen.getByRole('tab', { name: 'Conversation' })
    fireEvent.keyDown(conversation, { key: 'ArrowRight' })
    expect(screen.getByRole('tab', { name: 'Project' }).getAttribute('aria-selected')).toBe('true')
  })

  it('saves versioned Project metadata and renders a durable receipt', async () => {
    render(
      <ProjectAgentWorkbench projectId="project-1">
        <div>Conversation</div>
      </ProjectAgentWorkbench>,
    )

    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Atlas Next' } })
    expect(screen.getByText('dirty')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Save metadata' }))

    expect(await screen.findByText('Project metadata saved at version 5')).toBeTruthy()
    expect(updateProject).toHaveBeenCalledWith({
      projectId: 'project-1',
      body: { version: 4, name: 'Atlas Next' },
    })
  })

  it('admits only one write when the same form is submitted twice before the request settles', () => {
    updateProject.mockReturnValueOnce(new Promise(() => {}))
    render(
      <ProjectAgentWorkbench projectId="project-1">
        <div>Conversation</div>
      </ProjectAgentWorkbench>,
    )

    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Atlas Once' } })
    const form = screen.getByRole('button', { name: 'Save metadata' }).closest('form')
    expect(form).toBeTruthy()
    fireEvent.submit(form!)
    fireEvent.submit(form!)

    expect(updateProject).toHaveBeenCalledTimes(1)
  })

  it('announces optimistic-concurrency conflicts', async () => {
    updateProject.mockRejectedValueOnce(new ApiError('version conflict', 409))
    render(
      <ProjectAgentWorkbench projectId="project-1">
        <div>Conversation</div>
      </ProjectAgentWorkbench>,
    )

    fireEvent.change(screen.getByLabelText('Project name'), { target: { value: 'Stale edit' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save metadata' }))
    expect(await screen.findByRole('alert')).toBeTruthy()
    expect(screen.getByText(/changed elsewhere/i)).toBeTruthy()
  })
})
