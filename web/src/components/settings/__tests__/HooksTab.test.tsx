import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { HooksTab } from '@/components/settings/HooksTab'

vi.mock('@/components/settings/ProjectHooksSection', () => ({
  ProjectHooksSection: () => <div>Generic Project hooks</div>,
}))

describe('HooksTab', () => {
  it('shows target Project hooks without workflow lifecycle controls', () => {
    render(<HooksTab projectId="p1" projectIsLoading={false} />)

    expect(screen.getByText('Generic Project hooks')).toBeTruthy()
    expect(screen.queryByText('Lifecycle Hooks')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Test' })).toBeNull()
  })
})
