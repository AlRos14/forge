import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { PlanDocument } from '@/components/plan-document'
import type { PlanArtifactDetail } from '@/types/generated'

const artifact: PlanArtifactDetail = {
  artifact_id: 'artifact-1',
  task_id: 'task-1',
  producer_execution_id: 'execution-1',
  producer: { kind: 'agent', id: 'agent-1' },
  content_digest: 'sha256:plan-document',
  markdown: '# Implementation map\n\nThe plan Artifact is visible immediately.\n\n- [ ] Ship it',
  items: [{ checked: false, label: 'Ship it', nesting_level: 0, line_number: 5 }],
  warnings: [],
  created_at: '2026-09-02T09:00:00Z',
}

describe('PlanDocument', () => {
  it('renders the complete Markdown plan without a disclosure', () => {
    render(<PlanDocument artifact={artifact} />)

    expect(screen.getByRole('heading', { name: 'Implementation map' })).toBeTruthy()
    expect(screen.getByText('The plan Artifact is visible immediately.')).toBeTruthy()
    expect(screen.queryByText(/completed$/)).toBeNull()
    expect(screen.queryByText('Full plan')).toBeNull()
  })

  it('renders Artifact provenance and parser warnings', () => {
    render(<PlanDocument artifact={{ ...artifact, warnings: ['Malformed checklist item'] }} />)

    expect(screen.getByText('artifact-1')).toBeTruthy()
    expect(screen.getByText('agent:agent-1')).toBeTruthy()
    expect(screen.getByText('execution-1')).toBeTruthy()
    expect(screen.getByText('Malformed checklist item')).toBeTruthy()
  })
})
