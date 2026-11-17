import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import type { HumanDecisionResponse, Task } from '@/types/generated'
import { TaskHumanDecisionCard } from './task-human-decisions-panel'

const answer = vi.hoisted(() => ({ mutateAsync: vi.fn(), isPending: false }))

vi.mock('@/api/hooks', () => ({
  useAnswerHumanDecision: () => answer,
  useTaskHumanDecisionsQuery: () => ({ data: { items: [] }, isLoading: false, isError: false }),
}))
vi.mock('sonner', () => ({ toast: { success: vi.fn(), error: vi.fn() } }))

const task = { id: 'task-1', project_id: 'project-1' } as unknown as Task

function decision(overrides: Partial<HumanDecisionResponse> = {}): HumanDecisionResponse {
  return {
    id: 'decision-1',
    task_id: 'task-1',
    source_kind: 'gate_evaluation',
    source_id: 'eval-1',
    raised_by_execution_id: null,
    authorization_version: 1,
    question: 'Ship with the known failure?',
    options: [
      { id: 'a', label: 'Fix first', description: 'Rework', consequence: 'Delays', authority_effect: 'none' },
      { id: 'b', label: 'Widen scope', description: 'More', consequence: 'Bigger', authority_effect: 'scope' },
    ],
    recommended_option_id: 'a',
    recommendation_rationale: 'It is the safest path',
    evidence_links: [{ kind: 'gate_evaluation', id: 'eval-1', digest: null }],
    max_authority_effect: 'scope',
    state: 'open',
    answered_by_user_id: null,
    answered_option_id: null,
    answer_note: null,
    answered_at: null,
    resumes_request_id: null,
    resume_request_id: null,
    version: 3,
    created_at: '',
    updated_at: '',
    ...overrides,
  }
}

describe('TaskHumanDecisionCard', () => {
  beforeEach(() => {
    answer.mutateAsync.mockReset()
  })

  it('submits the recommended option with the expected version', async () => {
    answer.mutateAsync.mockResolvedValue({})
    render(<TaskHumanDecisionCard task={task} decision={decision()} canAuthority />)
    expect(screen.getByText('Recommended')).toBeTruthy()
    expect(screen.getByText(/safest path/)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Record answer' }))
    await waitFor(() => expect(answer.mutateAsync).toHaveBeenCalled())
    const call = answer.mutateAsync.mock.calls[0][0]
    expect(call.decisionId).toBe('decision-1')
    expect(call.body.expected_version).toBe(3)
    expect(call.body.option_id).toBe('a')
  })

  it('shows a conflict message on 409 instead of a generic failure', async () => {
    answer.mutateAsync.mockRejectedValue(
      new ApiError(JSON.stringify({ code: 'human_decision.version_conflict', message: 'stale' }), 409),
    )
    render(<TaskHumanDecisionCard task={task} decision={decision()} canAuthority />)
    fireEvent.click(screen.getByRole('button', { name: 'Record answer' }))
    expect((await screen.findByRole('alert')).textContent).toMatch(/changed while you were reading/)
  })

  it('blocks authority options for non owner/admin', () => {
    render(<TaskHumanDecisionCard task={task} decision={decision()} canAuthority={false} />)
    expect(screen.getByText(/Only a Project owner or admin/)).toBeTruthy()
  })

  it('shows the answered state without an answer action', () => {
    render(
      <TaskHumanDecisionCard
        task={task}
        decision={decision({ state: 'answered', answered_option_id: 'a' })}
        canAuthority
      />,
    )
    expect(screen.queryByRole('button', { name: 'Record answer' })).toBeNull()
    expect(screen.getByText('Chosen')).toBeTruthy()
  })
})
