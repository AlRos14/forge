import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { PlanDocument } from '@/components/plan-document'
import type { PlanArtifactDetail } from '@/types/generated'

const artifact: PlanArtifactDetail = {
  revision_id: 'revision-1',
  revision: 1,
  checkpoint: 'planner_ready',
  content_digest: 'sha256:plan-document',
  markdown: '# Implementation map\n\nThe canonical plan is visible immediately.\n\n- [ ] Ship it',
  items: [{ checked: false, label: 'Ship it', nesting_level: 0, line_number: 5 }],
  warnings: [],
  last_modified: '2026-09-02T09:00:00Z',
}

describe('PlanDocument', () => {
  it('renders the complete Markdown plan without a disclosure', () => {
    render(<PlanDocument artifact={artifact} />)

    expect(screen.getByRole('heading', { name: 'Implementation map' })).toBeTruthy()
    expect(screen.getByText('The canonical plan is visible immediately.')).toBeTruthy()
    expect(screen.queryByText(/completed$/)).toBeNull()
    expect(screen.queryByText('Full plan')).toBeNull()
  })

  it('renders revision metadata and parser warnings', () => {
    render(<PlanDocument artifact={{ ...artifact, warnings: ['Malformed checklist item'] }} />)

    expect(screen.getByText(/Revision 1 · planner_ready · sha256:plan-/)).toBeTruthy()
    expect(screen.getByText('Malformed checklist item')).toBeTruthy()
  })
})
