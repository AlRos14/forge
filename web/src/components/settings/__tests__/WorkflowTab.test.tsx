import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { WorkflowTab } from '@/components/settings/WorkflowTab'
import type { WorkflowDefinition } from '@/types/generated'

const hooks = vi.hoisted(() => ({
  useWorkflowQuery: vi.fn(),
  useWorkflowPromptBuildersQuery: vi.fn(),
  useWorkflowTemplatesQuery: vi.fn(),
  useUpdateWorkflow: vi.fn(),
}))

vi.mock('@/api/hooks', () => hooks)

const updateWorkflow = { isPending: false, mutate: vi.fn() }

function workflowFixture(): WorkflowDefinition {
  return {
    roles: [
      { name: 'coder', display_name: 'Coder', description: '' },
      { name: 'reviewer', display_name: 'Reviewer', description: '' },
    ],
    states: [
      {
        name: 'review',
        kind: 'gate',
        column: 'Review',
        display_name: 'Review',
        role: 'reviewer',
        hooks: { before_exit: [], on_exit: [], before_enter: [], on_enter: [], after_enter: [] },
        cleanup: null,
        gate_config: null,
        dispatch: null,
        config: {},
        triggers: {
          reject: {
            to: 'in_progress',
            dispatch: {
              builder: 'coder.default.v1',
              execution_policy: 'resume_latest_target_role_thread',
              prompt: { user_append: 'Address review feedback.' },
            },
          },
        },
      },
      {
        name: 'in_progress',
        kind: 'active',
        column: 'In Progress',
        display_name: 'In Progress',
        role: 'coder',
        hooks: { before_exit: [], on_exit: [], before_enter: [], on_enter: [], after_enter: [] },
        cleanup: null,
        gate_config: null,
        dispatch: null,
        triggers: {},
        config: {},
      },
    ],
    configuration: [],
    cancellation_state: null,
  } as unknown as WorkflowDefinition
}

describe('WorkflowTab dispatch editor', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    hooks.useWorkflowQuery.mockReturnValue({ data: workflowFixture(), isLoading: false })
    hooks.useWorkflowPromptBuildersQuery.mockReturnValue({
      data: [
        {
          id: 'coder.default.v1',
          label: 'Coder (Default)',
          compatible_role_hints: ['coder'],
          description: 'Implementation-focused prompt.',
        },
      ],
    })
    hooks.useWorkflowTemplatesQuery.mockReturnValue({ data: [], isLoading: false })
    hooks.useUpdateWorkflow.mockReturnValue(updateWorkflow)
  })

  it('renders trigger dispatch editor without a role picker', async () => {
    render(<WorkflowTab projectId="p1" workflowTemplateName={undefined} />)

    expect(screen.getByText('Dispatch')).toBeTruthy()
    fireEvent.click(await screen.findByText('Trigger dispatch'))
    fireEvent.click(await screen.findByText('reject'))
    expect(screen.getByRole('button', { name: /select target phase/i }).textContent).toContain(
      'In Progress',
    )
    expect(screen.getByDisplayValue('Address review feedback.')).toBeTruthy()
    expect(screen.queryByLabelText(/role/i)).toBeNull()
  })

  it('loads prompt builder options from registry-backed query', async () => {
    render(<WorkflowTab projectId="p1" workflowTemplateName={undefined} />)

    fireEvent.click((await screen.findAllByRole('button', { name: /select prompt builder/i }))[0])
    expect(screen.getByRole('option', { name: 'Coder (Default)' })).toBeTruthy()
  })
})
