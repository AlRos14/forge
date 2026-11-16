import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/client'
import type { PrFeedbackPolicy } from '@/types/generated'
import { RepoPrFeedbackPolicyEditor } from '@/components/settings/RepoPrFeedbackPolicyEditor'

const policyQuery = vi.hoisted(() => ({
  data: undefined as PrFeedbackPolicy | undefined,
  isLoading: false,
  isError: false,
  refetch: vi.fn(),
}))
const policyMutation = vi.hoisted(() => ({ mutateAsync: vi.fn(), isPending: false }))

vi.mock('@/api/hooks', () => ({
  useRepoPrFeedbackPolicyQuery: () => policyQuery,
  useUpdateRepoPrFeedbackPolicy: () => policyMutation,
}))

const initialPolicy: PrFeedbackPolicy = {
  schema_version: 1,
  ingest: false,
  trusted_author_associations: ['OWNER', 'MEMBER', 'COLLABORATOR'],
  trusted_logins: [],
  max_items_per_poll: 50,
  push_corrections: 'human_approval',
  post_summary: 'off',
}

describe('RepoPrFeedbackPolicyEditor', () => {
  beforeEach(() => {
    policyQuery.data = initialPolicy
    policyQuery.isLoading = false
    policyQuery.isError = false
    policyQuery.refetch.mockReset()
    policyMutation.mutateAsync.mockReset()
    policyMutation.isPending = false
  })

  it('saves the generated policy shape with expected_updated_at', async () => {
    policyMutation.mutateAsync.mockResolvedValue({ ...initialPolicy, ingest: true })
    render(<RepoPrFeedbackPolicyEditor repoId="repo-1" />)

    fireEvent.click(screen.getByLabelText('Ingest pull request feedback'))
    fireEvent.change(screen.getByLabelText('Maximum items per poll'), {
      target: { value: '25' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save policy' }))

    await waitFor(() => expect(policyMutation.mutateAsync).toHaveBeenCalled())
    expect(policyMutation.mutateAsync).toHaveBeenCalledWith({
      repoId: 'repo-1',
      body: {
        expected_updated_at: null,
        policy: { ...initialPolicy, ingest: true, max_items_per_poll: 25 },
      },
    })
  })

  it('prompts to reload when the policy version conflicts', async () => {
    policyMutation.mutateAsync.mockRejectedValue(
      new ApiError(
        JSON.stringify({ code: 'pr_feedback.policy_version_conflict', message: 'stale' }),
        409,
      ),
    )
    const latestPolicy = { ...initialPolicy, ingest: true, max_items_per_poll: 30 }
    policyQuery.refetch.mockResolvedValue({ data: latestPolicy })
    render(<RepoPrFeedbackPolicyEditor repoId="repo-1" />)

    fireEvent.click(screen.getByRole('button', { name: 'Save policy' }))
    expect((await screen.findByRole('alert')).textContent).toMatch(/changed while you were editing/)
    fireEvent.click(screen.getByRole('button', { name: 'Reload latest policy' }))

    await waitFor(() => expect(policyQuery.refetch).toHaveBeenCalled())
    expect(screen.getByLabelText('Maximum items per poll')).toHaveProperty('value', '30')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('shows the typed validation error for an invalid policy', async () => {
    policyMutation.mutateAsync.mockRejectedValue(
      new ApiError(
        JSON.stringify({ code: 'pr_feedback.policy_invalid', message: 'invalid' }),
        400,
      ),
    )
    render(<RepoPrFeedbackPolicyEditor repoId="repo-1" />)

    fireEvent.click(screen.getByRole('button', { name: 'Save policy' }))

    expect((await screen.findByRole('alert')).textContent).toMatch(/Policy values are invalid/)
  })
})
