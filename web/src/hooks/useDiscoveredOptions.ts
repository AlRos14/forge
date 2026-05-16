import { useQuery } from '@tanstack/react-query'
import { apiFetch } from '@/api/client'
import { qk } from '@/api/query-keys'

export type DiscoveredModelOption = {
  id: string
  displayName: string
  provider: string | null
  reasoningOptions: string[]
}

export type ReasoningOption = {
  id: string
  label: string
}

export type DiscoveredExecutionOptions = {
  models: DiscoveredModelOption[]
  reasoningOptions: ReasoningOption[]
  permissionPolicies: string[]
}

type AgentDiscoveredOptionsResponse = {
  models?: Array<{ id: string; name?: string; reasoning_options?: string[] }> | string[]
  permission_policies?: string[]
  cli_specific?: unknown
}

function titleCase(value: string): string {
  if (value === 'xhigh') return 'XHigh'
  return value
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ')
}

function providerForModel(modelId: string): string | null {
  const lower = modelId.toLowerCase()
  if (lower.includes('claude')) return 'Anthropic'
  if (lower.includes('gpt') || lower.includes('codex') || lower.includes('o3')) return 'OpenAI'
  return null
}

function uniqueStrings(values: unknown): string[] {
  if (!Array.isArray(values)) return []
  return Array.from(
    new Set(values.filter((value): value is string => typeof value === 'string' && Boolean(value))),
  )
}

function reasoningFromCliSpecific(cliSpecific: unknown, hasModels: boolean): string[] {
  if (cliSpecific && typeof cliSpecific === 'object') {
    const record = cliSpecific as Record<string, unknown>
    for (const key of ['reasoning_efforts', 'reasoning_options', 'model_reasoning_efforts']) {
      const values = uniqueStrings(record[key])
      if (values.length > 0) return values
    }
  }
  return hasModels ? ['low', 'medium', 'high'] : []
}

function normalizeDiscoveredOptions(
  response: AgentDiscoveredOptionsResponse,
): DiscoveredExecutionOptions {
  const allReasoningIds = new Set<string>()
  const models: DiscoveredModelOption[] = []

  if (Array.isArray(response.models)) {
    for (const entry of response.models) {
      if (typeof entry === 'string') {
        models.push({ id: entry, displayName: entry, provider: providerForModel(entry), reasoningOptions: [] })
      } else if (entry && typeof entry === 'object') {
        const id = entry.id ?? ''
        const reasoningOptions = entry.reasoning_options ?? []
        reasoningOptions.forEach((r) => allReasoningIds.add(r))
        models.push({
          id,
          displayName: entry.name ?? id,
          provider: providerForModel(id),
          reasoningOptions,
        })
      }
    }
  }

  const reasoningIds =
    allReasoningIds.size > 0
      ? Array.from(allReasoningIds)
      : reasoningFromCliSpecific(response.cli_specific, models.length > 0)

  const policies = uniqueStrings(response.permission_policies)

  return {
    models,
    reasoningOptions: reasoningIds.map((id) => ({ id, label: titleCase(id) })),
    permissionPolicies: policies.length > 0 ? policies : ['auto', 'supervised', 'plan'],
  }
}

export function useDiscoveredOptions(
  agentId: string | null | undefined,
  executorType?: string | null,
) {
  const hasAgentId = Boolean(agentId)
  const hasExecutorType = Boolean(executorType)

  return useQuery({
    queryKey: hasAgentId
      ? qk.agentDiscoveredOptions(agentId ?? '')
      : qk.executorDiscoveredOptions(executorType ?? ''),
    queryFn: async () => {
      const path = hasAgentId
        ? `/agents/${agentId}/discovered-options`
        : `/executor-types/${executorType}/discovered-options`
      const response = await apiFetch<AgentDiscoveredOptionsResponse>(path)
      return normalizeDiscoveredOptions(response)
    },
    enabled: hasAgentId || hasExecutorType,
    staleTime: 5 * 60 * 1000,
  })
}
