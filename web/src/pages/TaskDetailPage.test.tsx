import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { ReviewExecutionResponse } from '@/types/generated'
import { ReviewExecutionCard } from './TaskDetailPage'

function makeReview(
  actor_ref: ReviewExecutionResponse['execution']['actor_ref'],
  status: ReviewExecutionResponse['execution']['status'] = 'running',
  report: ReviewExecutionResponse['report'] = null,
): ReviewExecutionResponse {
  return {
    execution: {
      id: 'execution-1',
      role: 'reviewer',
      purpose: 'review',
      status,
      actor_ref,
      workspace_id: null,
      before_sha: null,
      after_sha: null,
    } as ReviewExecutionResponse['execution'],
    report,
  }
}

function renderCard(review: ReviewExecutionResponse, currentUserId = 'user-1') {
  return render(
    <ReviewExecutionCard
      review={review}
      currentUserId={currentUserId}
      validations={[]}
      form={{
        verdict: 'pass',
        summary: 'Ready to submit',
        criteria: [],
        findings: [],
        questions: [],
        evidence_ids: [],
        artifact_ids: [],
      }}
      onFormChange={vi.fn()}
      onSubmit={vi.fn()}
      submitting={false}
    />,
  )
}

describe('ReviewExecutionCard authorization', () => {
  it('offers Human submission only for the current Human Actor', () => {
    renderCard(makeReview({ kind: 'human', id: 'user-1' }))

    expect(screen.getByText('Execution execution-1')).toBeTruthy()
    expect(screen.getByLabelText('Review summary')).toBeTruthy()
    const submit = screen.getByRole('button', { name: 'Submit ReviewReport for this Execution' })
    expect((submit as HTMLButtonElement).disabled).toBe(false)
  })

  it('keeps another Human Execution visible as history without a submit action', () => {
    renderCard(makeReview({ kind: 'human', id: 'user-2' }))

    expect(screen.getByText('Execution execution-1')).toBeTruthy()
    expect(screen.getByText('Actor human:user-2 · Workspace none · base none · head none')).toBeTruthy()
    expect(screen.queryByLabelText('Review summary')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' })).toBeNull()
  })

  it('never offers Human submission for an Agent Review Execution', () => {
    renderCard(makeReview({ kind: 'agent', id: 'agent-1' }))

    expect(screen.getByText('Execution execution-1')).toBeTruthy()
    expect(screen.getByText('Actor agent:agent-1 · Workspace none · base none · head none')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' })).toBeNull()
  })

  it('keeps a completed ReviewReport visible without a submit action', () => {
    const report = { id: 'report-1', content: 'Review passed.' } as NonNullable<
      ReviewExecutionResponse['report']
    >
    renderCard(makeReview({ kind: 'human', id: 'user-1' }, 'completed', report))

    expect(screen.getByText('Review passed.')).toBeTruthy()
    expect(screen.queryByLabelText('Review summary')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Submit ReviewReport for this Execution' })).toBeNull()
  })
})
