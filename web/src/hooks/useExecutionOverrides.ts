import { useCallback, useEffect, useMemo, useState } from 'react'
import { useDiscoveredOptions } from '@/hooks/useDiscoveredOptions'
import {
  getRecentExecutionSelection,
  type ExecutionConfigSelection,
  type ExecutionOverrides,
} from '@/lib/execution-config-storage'
import type { Agent } from '@/types/generated'

export type ExecutionConfigValue = {
  agentId: string | null
  modelId: string | null
  reasoningEffort: string | null
  permissionPolicy: string | null
  selection: ExecutionConfigSelection
  overrides: ExecutionOverrides | undefined
}

export type AgentDefaults = Pick<Agent, 'model' | 'reasoning_effort' | 'permission_policy'>

export type UseExecutionOverridesOptions = {
  initialAgentId?: string | null
  initialOverrides?: Partial<ExecutionConfigSelection> | null
  agentDefaults?: AgentDefaults | null
  useRecentSelections?: boolean
}

function valueOrNull(value: string | null | undefined): string | null {
  return value && value.trim() ? value : null
}

function defaultsForAgent(
  agentId: string | null,
  initialAgentId: string | null | undefined,
  initialOverrides: Partial<ExecutionConfigSelection> | null | undefined,
  agentDefaults: AgentDefaults | null | undefined,
  useRecentSelections: boolean,
): ExecutionConfigSelection {
  if (!agentId) {
    return { modelId: null, reasoningEffort: null, permissionPolicy: null }
  }

  if (useRecentSelections) {
    const recent = getRecentExecutionSelection(agentId)
    if (recent) {
      return {
        modelId: recent.lastModelId,
        reasoningEffort: recent.lastReasoningEffort,
        permissionPolicy: recent.lastPermissionPolicy,
      }
    }
  }

  if (agentId === initialAgentId && initialOverrides) {
    return {
      modelId: valueOrNull(initialOverrides.modelId),
      reasoningEffort: valueOrNull(initialOverrides.reasoningEffort),
      permissionPolicy: valueOrNull(initialOverrides.permissionPolicy),
    }
  }

  return {
    modelId: valueOrNull(agentDefaults?.model),
    reasoningEffort: valueOrNull(agentDefaults?.reasoning_effort),
    permissionPolicy: valueOrNull(agentDefaults?.permission_policy),
  }
}

function computeOverrides(
  selection: ExecutionConfigSelection,
  agentDefaults: AgentDefaults | null | undefined,
): ExecutionOverrides | undefined {
  const overrides: ExecutionOverrides = {}
  const agentModel = valueOrNull(agentDefaults?.model)
  const agentReasoning = valueOrNull(agentDefaults?.reasoning_effort)
  const agentPolicy = valueOrNull(agentDefaults?.permission_policy)

  if (selection.modelId && selection.modelId !== agentModel) overrides.model_id = selection.modelId
  if (selection.reasoningEffort && selection.reasoningEffort !== agentReasoning)
    overrides.reasoning_effort = selection.reasoningEffort
  if (selection.permissionPolicy && selection.permissionPolicy !== agentPolicy)
    overrides.permission_policy = selection.permissionPolicy

  return Object.keys(overrides).length > 0 ? overrides : undefined
}

export function useExecutionOverrides({
  initialAgentId,
  initialOverrides,
  agentDefaults,
  useRecentSelections = true,
}: UseExecutionOverridesOptions) {
  const normalizedInitialAgentId = valueOrNull(initialAgentId)
  const [agentId, setAgentIdState] = useState<string | null>(normalizedInitialAgentId)
  const [selection, setSelection] = useState<ExecutionConfigSelection>(() =>
    defaultsForAgent(
      normalizedInitialAgentId,
      normalizedInitialAgentId,
      initialOverrides,
      agentDefaults,
      useRecentSelections,
    ),
  )
  const discoveredOptions = useDiscoveredOptions(agentId)

  useEffect(() => {
    setAgentIdState(normalizedInitialAgentId)
  }, [normalizedInitialAgentId])

  useEffect(() => {
    setSelection(
      defaultsForAgent(
        agentId,
        normalizedInitialAgentId,
        initialOverrides,
        agentDefaults,
        useRecentSelections,
      ),
    )
  }, [agentDefaults, agentId, initialOverrides, normalizedInitialAgentId, useRecentSelections])

  const setAgentId = useCallback((nextAgentId: string | null) => {
    setAgentIdState(valueOrNull(nextAgentId))
  }, [])

  const setModelId = useCallback((modelId: string | null) => {
    setSelection((current) => ({
      ...current,
      modelId: valueOrNull(modelId),
      reasoningEffort: null,
    }))
  }, [])

  const setReasoningEffort = useCallback((reasoningEffort: string | null) => {
    setSelection((current) => ({ ...current, reasoningEffort: valueOrNull(reasoningEffort) }))
  }, [])

  const setPermissionPolicy = useCallback((permissionPolicy: string | null) => {
    setSelection((current) => ({ ...current, permissionPolicy: valueOrNull(permissionPolicy) }))
  }, [])

  const resetToAgentDefaults = useCallback(() => {
    setSelection({
      modelId: valueOrNull(agentDefaults?.model),
      reasoningEffort: valueOrNull(agentDefaults?.reasoning_effort),
      permissionPolicy: valueOrNull(agentDefaults?.permission_policy),
    })
  }, [agentDefaults])

  const hasCustomValues = useMemo(() => {
    const agentModel = valueOrNull(agentDefaults?.model)
    const agentReasoning = valueOrNull(agentDefaults?.reasoning_effort)
    const agentPolicy = valueOrNull(agentDefaults?.permission_policy)
    return (
      (selection.modelId !== null && selection.modelId !== agentModel) ||
      (selection.reasoningEffort !== null && selection.reasoningEffort !== agentReasoning) ||
      (selection.permissionPolicy !== null && selection.permissionPolicy !== agentPolicy)
    )
  }, [agentDefaults, selection])

  const value = useMemo<ExecutionConfigValue>(() => {
    const overrides = computeOverrides(selection, agentDefaults)
    return {
      agentId,
      modelId: selection.modelId,
      reasoningEffort: selection.reasoningEffort,
      permissionPolicy: selection.permissionPolicy,
      selection,
      overrides,
    }
  }, [agentDefaults, agentId, selection])

  return {
    ...value,
    discoveredOptions,
    hasCustomValues,
    setAgentId,
    setModelId,
    setReasoningEffort,
    setPermissionPolicy,
    resetToAgentDefaults,
  }
}
