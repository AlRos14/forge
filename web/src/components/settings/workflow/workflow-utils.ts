import type { StateDefinition, WorkflowConfigField, WorkflowDefinition } from '@/types/generated'

export type WorkflowRecord = Record<string, unknown>
export type DispatchSource = { dispatch?: unknown }
export type MutableDispatchSource = DispatchSource & { dispatch?: unknown }
export type TriggerRecord = WorkflowRecord & { to?: unknown; dispatch?: unknown }
export type ColumnGroup = { column: string; displayName: string; states: StateDefinition[] }

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function parseWorkflowFieldValue(field: WorkflowConfigField, value: string): unknown {
  if (field.value_type === 'text') return value

  const trimmed = value.trim()
  const parsed = Number(trimmed)
  if (!trimmed || !Number.isInteger(parsed)) {
    throw new Error(`${field.label} must be an integer`)
  }
  if (field.min != null && parsed < field.min) {
    throw new Error(`${field.label} must be ${field.min} or greater`)
  }
  return parsed
}

export function cloneWorkflow(workflow: WorkflowDefinition): WorkflowDefinition {
  return JSON.parse(JSON.stringify(workflow)) as WorkflowDefinition
}

export function configurationFields(workflow: WorkflowDefinition | undefined): WorkflowConfigField[] {
  return workflow?.configuration ?? []
}

export function stateConfigValue(config: Record<string, unknown>, path: string[]): unknown {
  let cursor: unknown = config
  for (const segment of path) {
    if (!isRecord(cursor)) return undefined
    cursor = cursor[segment]
  }
  return cursor
}

export function workflowFieldValue(workflow: WorkflowDefinition, field: WorkflowConfigField): string {
  const state = workflow.states.find((candidate) => candidate.name === field.binding.state)
  let value: unknown
  if (field.binding.type === 'gate_config') {
    value = isRecord(state?.gate_config)
      ? (state.gate_config as Record<string, unknown>)[field.binding.field]
      : undefined
  } else {
    value = state ? stateConfigValue(state.config, field.binding.path) : undefined
  }
  if (field.value_type === 'text') {
    if (typeof value === 'string') return value
    return typeof field.default_value === 'string' ? field.default_value : ''
  }
  if (typeof value === 'number' && Number.isFinite(value)) return String(Math.trunc(value))
  return typeof field.default_value === 'number' ? String(field.default_value) : ''
}

export function setStateConfigValue(config: Record<string, unknown>, path: string[], value: unknown) {
  let cursor = config
  for (const segment of path.slice(0, -1)) {
    const next = cursor[segment]
    if (isRecord(next)) {
      cursor = next
    } else {
      const created: Record<string, unknown> = {}
      cursor[segment] = created
      cursor = created
    }
  }
  const leaf = path[path.length - 1]
  if (leaf) cursor[leaf] = value
}

export function setWorkflowFieldValue(
  workflow: WorkflowDefinition,
  field: WorkflowConfigField,
  value: unknown,
): boolean {
  const state = workflow.states.find((candidate) => candidate.name === field.binding.state)
  if (!state) return false
  if (field.binding.type === 'gate_config') {
    if (!state.gate_config) return false
    if (typeof value !== 'number') return false
    ;(state.gate_config as unknown as Record<string, unknown>)[field.binding.field] = value
    return true
  }
  const config = isRecord(state.config) ? { ...state.config } : {}
  setStateConfigValue(config, field.binding.path, value)
  state.config = config
  return true
}

export function asRecord(value: unknown): WorkflowRecord | null {
  if (!isRecord(value)) return null
  return value
}

export function stateRecords(workflow: WorkflowDefinition | undefined): StateDefinition[] {
  if (!workflow) return []
  return workflow.states.filter((state) => typeof state.name === 'string')
}

export function readDispatchBuilder(source: DispatchSource | null): string {
  const dispatch = asRecord(source?.dispatch)
  return typeof dispatch?.builder === 'string' ? dispatch.builder : ''
}

export function readDispatchExecutionPolicy(source: DispatchSource | null): string {
  const dispatch = asRecord(source?.dispatch)
  return typeof dispatch?.execution_policy === 'string' ? dispatch.execution_policy : ''
}

export function readDispatchInstructions(source: DispatchSource | null): string {
  const dispatch = asRecord(source?.dispatch)
  const prompt = asRecord(dispatch?.prompt)
  return typeof prompt?.user_append === 'string' ? prompt.user_append : ''
}

export function triggerRecords(state: StateDefinition): Array<{ name: string; trigger: TriggerRecord }> {
  const triggers = asRecord(state.triggers)
  if (!triggers) return []
  return Object.entries(triggers)
    .filter(([, value]) => isRecord(value))
    .map(([name, trigger]) => ({ name, trigger: trigger as TriggerRecord }))
}

export function mutateDispatchField(
  object: MutableDispatchSource,
  field: 'builder' | 'execution_policy',
  value: string,
) {
  const nextDispatch = asRecord(object.dispatch) ?? {}
  if (value) {
    nextDispatch[field] = value
  } else {
    delete nextDispatch[field]
  }
  if (Object.keys(nextDispatch).length > 0) {
    object.dispatch = nextDispatch
  } else {
    delete object.dispatch
  }
}

export function mutateDispatchInstructions(object: MutableDispatchSource, value: string) {
  const nextDispatch = asRecord(object.dispatch) ?? {}
  const nextPrompt = asRecord(nextDispatch.prompt) ?? {}
  if (value.trim()) {
    nextPrompt.user_append = value
  } else {
    delete nextPrompt.user_append
  }
  if (Object.keys(nextPrompt).length > 0) {
    nextDispatch.prompt = nextPrompt
  } else {
    delete nextDispatch.prompt
  }
  if (Object.keys(nextDispatch).length > 0) {
    object.dispatch = nextDispatch
  } else {
    delete object.dispatch
  }
}

export function toTitleCase(str: string): string {
  return str.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase())
}

export function groupStatesByColumn(states: StateDefinition[]): ColumnGroup[] {
  const groups: ColumnGroup[] = []
  const seen = new Map<string, ColumnGroup>()
  for (const state of states) {
    let group = seen.get(state.column)
    if (!group) {
      group = { column: state.column, displayName: toTitleCase(state.column), states: [] }
      groups.push(group)
      seen.set(state.column, group)
    }
    group.states.push(state)
  }
  return groups
}
