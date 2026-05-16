import type { Agent, CreateAgentRequest } from '@/types/generated'

export type AgentFormState = {
  name: string
  description: string
  executor_type: string
  model: string
  reasoning_effort: string
  permission_policy: string
  prompt_template: string
  daemon_id: string
  capabilities: string
  max_concurrent_tasks: string
  config_json: string
}

export const emptyForm: AgentFormState = {
  name: '',
  description: '',
  executor_type: 'claude_code',
  model: '',
  reasoning_effort: '',
  permission_policy: 'supervised',
  prompt_template: '',
  daemon_id: '',
  capabilities: '',
  max_concurrent_tasks: '1',
  config_json: '{}',
}

export function agentToForm(agent: Agent): AgentFormState {
  return {
    name: agent.name,
    description: agent.description ?? '',
    executor_type: agent.executor_type,
    model: agent.model ?? '',
    reasoning_effort: agent.reasoning_effort ?? '',
    permission_policy: agent.permission_policy ?? '',
    prompt_template: agent.prompt_template ?? '',
    daemon_id: agent.daemon_id ?? '',
    capabilities: agent.capabilities.join(', '),
    max_concurrent_tasks: String(agent.max_concurrent_tasks),
    config_json: JSON.stringify(agent.config_json ?? {}, null, 2),
  }
}

export function parseForm(
  form: AgentFormState,
  options: { includeDaemon?: boolean } = {},
): Omit<CreateAgentRequest, 'name' | 'executor_type'> {
  let config: Record<string, unknown>
  try {
    const parsed = JSON.parse(form.config_json || '{}')
    config = parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {}
  } catch {
    throw new Error('Config JSON must be a valid JSON object')
  }

  const maxConcurrent = Number(form.max_concurrent_tasks)
  if (!Number.isInteger(maxConcurrent) || maxConcurrent < 1) {
    throw new Error('Max concurrent tasks must be a positive integer')
  }

  const includeDaemon = options.includeDaemon ?? true
  return {
    description: form.description.trim() || null,
    model: form.model.trim() || null,
    reasoning_effort: form.reasoning_effort.trim() || null,
    permission_policy: form.permission_policy.trim() || null,
    prompt_template: form.prompt_template.trim() || null,
    capabilities: form.capabilities
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean),
    config_json: config,
    ...(includeDaemon ? { daemon_id: form.daemon_id.trim() || null } : {}),
    max_concurrent_tasks: maxConcurrent,
  }
}

export function formatDate(value?: string | null): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString()
}

export function formatDuration(value?: number | null): string {
  if (value == null) return '—'
  if (value < 1000) return `${Math.round(value)}ms`
  const totalSeconds = Math.round(value / 1000)
  if (totalSeconds < 60) return `${totalSeconds}s`
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60
  if (minutes < 60) return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`
  const hours = Math.floor(minutes / 60)
  const remainingMinutes = minutes % 60
  return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`
}

export function formatPercent(value?: number | null): string {
  if (value == null) return '—'
  return `${Math.round(value * 100)}%`
}
