import { render, screen, fireEvent } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { PlanChecklist } from '@/components/plan-checklist'
import type { PlanArtifactDetail, PlanProgressSummary } from '@/types/generated'

const progress: PlanProgressSummary = {
  total: 3,
  completed: 2,
  remaining: 1,
  available: true,
  warnings: [],
}

const artifact: PlanArtifactDetail = {
  revision_id: 'revision-1',
  revision: 1,
  checkpoint: 'approved',
  content_digest: 'sha256:plan',
  markdown: '- [x] Prepare rollout\n  - [x] Validate daemon health\n  - [ ] Clear cleanup backlog',
  items: [
    { checked: true, label: 'Prepare rollout', nesting_level: 0, line_number: 1 },
    { checked: true, label: 'Validate daemon health', nesting_level: 1, line_number: 2 },
    { checked: false, label: 'Clear cleanup backlog', nesting_level: 1, line_number: 3 },
  ],
  warnings: [],
  last_modified: '2026-04-29T12:00:00Z',
}

describe('PlanChecklist', () => {
  it('renders progress bar with correct counts', () => {
    render(<PlanChecklist progress={progress} artifact={artifact} />)

    expect(screen.getByText('2/3 completed')).toBeTruthy()
  })

  it('hides checklist items by default', () => {
    render(<PlanChecklist progress={progress} artifact={artifact} />)

    expect(screen.queryByText('Prepare rollout')).toBeNull()
  })

  it('renders nested checklist items after expanding', () => {
    render(<PlanChecklist progress={progress} artifact={artifact} />)

    fireEvent.click(screen.getByRole('button', { expanded: false }))

    expect(screen.getAllByText('Prepare rollout')).toHaveLength(2)
    const validationEntries = screen.getAllByText('Validate daemon health')
    expect(validationEntries).toHaveLength(2)
    expect(screen.getAllByText('Clear cleanup backlog')).toHaveLength(2)
    expect(validationEntries[0].parentElement?.style.paddingLeft).toBe('14px')
  })

  it('renders empty state when unavailable', () => {
    render(
      <PlanChecklist
        progress={{ total: 0, completed: 0, remaining: 0, available: false, warnings: [] }}
      />,
    )

    expect(screen.getByText('No plan artifact available')).toBeTruthy()
  })
})
