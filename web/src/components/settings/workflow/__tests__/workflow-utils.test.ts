import { describe, expect, it } from 'vitest'
import {
  groupStatesByColumn,
  mutateDispatchField,
  mutateDispatchInstructions,
  parseWorkflowFieldValue,
  setWorkflowFieldValue,
} from '@/components/settings/workflow/workflow-utils'
import type { WorkflowConfigField, WorkflowDefinition } from '@/types/generated'

function numberField(): WorkflowConfigField {
  return {
    id: 'max_attempts',
    label: 'Max attempts',
    description: null,
    value_type: 'integer',
    min: 1,
    default_value: 2,
    binding: { type: 'state_config', state: 'active', path: ['retry', 'max_attempts'] },
  } as unknown as WorkflowConfigField
}

describe('workflow-utils', () => {
  it('parses integer fields with min constraint', () => {
    expect(parseWorkflowFieldValue(numberField(), '3')).toBe(3)
    expect(() => parseWorkflowFieldValue(numberField(), '0')).toThrow('Max attempts must be 1 or greater')
  })

  it('sets nested state config value through binding path', () => {
    const workflow = {
      states: [{ name: 'active', config: {}, gate_config: null }],
    } as unknown as WorkflowDefinition

    const didSet = setWorkflowFieldValue(workflow, numberField(), 5)

    expect(didSet).toBe(true)
    expect((workflow.states[0].config as Record<string, unknown>).retry).toEqual({ max_attempts: 5 })
  })

  it('mutates dispatch fields and clears empty dispatch object', () => {
    const source: { dispatch?: unknown } = { dispatch: { builder: 'a' } }
    mutateDispatchField(source, 'builder', '')
    expect(source.dispatch).toBeUndefined()

    mutateDispatchInstructions(source, '  note  ')
    expect(source.dispatch).toEqual({ prompt: { user_append: '  note  ' } })

    mutateDispatchInstructions(source, '   ')
    expect(source.dispatch).toBeUndefined()
  })

  it('groups states by insertion order and column', () => {
    const grouped = groupStatesByColumn([
      { name: 'a', column: 'in_progress' },
      { name: 'b', column: 'review' },
      { name: 'c', column: 'in_progress' },
    ] as any)

    expect(grouped.map((g) => g.column)).toEqual(['in_progress', 'review'])
    expect(grouped[0]?.states.map((s) => s.name)).toEqual(['a', 'c'])
  })
})
