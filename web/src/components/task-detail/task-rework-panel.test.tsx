import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { ReviewExecutionResponse, Task } from '@/types/generated'
import { TaskReworkPanel } from './task-rework-panel'

const mocks = vi.hoisted(() => ({ request: vi.fn() }))

vi.mock('@/api/hooks', () => ({
  useRequestRework: () => ({ mutateAsync: mocks.request, isPending: false }),
}))

const task = {
  id: 'task-1',
  version: 4,
  lifecycle: { task_id: 'task-1', state: 'active', version: 3 },
  task_roles: [
    { id: 'role-implementer', role: 'implementer', members: [] },
    { id: 'role-reviewer', role: 'reviewer', members: [] },
  ],
} as unknown as Task

const review = {
  execution: {
    id: 'execution-review-1',
    task_id: 'task-1',
    role: 'reviewer',
    status: 'completed',
  },
  report: {
    id: 'artifact-review-1',
    content: JSON.stringify({
      verdict: 'request_changes',
      subject: {
        reviewed_work_unit_id: 'unit-implementation',
        reviewed_work_unit_version: 2,
      },
    }),
  },
} as ReviewExecutionResponse

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
    })
    expect(retryBody).toEqual(firstBody)
    expect(retryBody.idempotency_key).toBeTruthy()
  })
})
