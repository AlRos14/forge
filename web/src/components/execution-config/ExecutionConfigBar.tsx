import { useEffect, useMemo } from 'react'
import { ArrowCounterClockwise } from '@phosphor-icons/react'
import { useAgentsQuery } from '@/api/hooks'
import { AgentSelector } from '@/components/execution-config/AgentSelector'
import { ModelSelector } from '@/components/execution-config/ModelSelector'
import { PolicySelector } from '@/components/execution-config/PolicySelector'
import { ReasoningSelector } from '@/components/execution-config/ReasoningSelector'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { useExecutionOverrides, type ExecutionConfigValue } from '@/hooks/useExecutionOverrides'
import { getReasoningOptionsForModel } from '@/hooks/useDiscoveredOptions'
import { getRecentExecutionSelection } from '@/lib/execution-config-storage'
import { cn } from '@/lib/cn'

export type { ExecutionConfigValue } from '@/hooks/useExecutionOverrides'

export function ExecutionConfigBar({
  initialAgentId,
  initialOverrides,
  executorTypeConstraint,
  disabled,
  useRecentSelections = true,
  showAgentSelector = true,
  showPolicySelector = true,
  className,
  onChange,
}: {
  initialAgentId?: string | null
  initialOverrides?: {
    modelId?: string | null
    reasoningEffort?: string | null
    permissionPolicy?: string | null
  } | null
  executorTypeConstraint?: string | null
  disabled?: boolean
  useRecentSelections?: boolean
  showAgentSelector?: boolean
  showPolicySelector?: boolean
  className?: string
  onChange: (value: ExecutionConfigValue) => void
}) {
  const agentsQuery = useAgentsQuery()
  const allAgents = agentsQuery.data?.items ?? []
  const visibleAgents = executorTypeConstraint
    ? allAgents.filter((a) => a.executor_type === executorTypeConstraint)
    : allAgents

  const config = useExecutionOverrides({
    initialAgentId,
    initialOverrides,
    agentDefaults: null,
    useRecentSelections,
  })

  const selectedAgent = visibleAgents.find((a) => a.id === config.agentId) ?? null
  const agentDefaults = useMemo(
    () =>
      selectedAgent
        ? {
            model: selectedAgent.model,
            reasoning_effort: selectedAgent.reasoning_effort,
            permission_policy: selectedAgent.permission_policy,
          }
        : null,
    [selectedAgent],
  )

  const discoveredOptions = config.discoveredOptions.data
  const recentModelIds = useMemo(
    () => getRecentExecutionSelection(config.agentId)?.recentModels.map((m) => m.modelId) ?? [],
    [config.agentId],
  )

  const reasoningOptionsForModel = useMemo(
    () => getReasoningOptionsForModel(discoveredOptions, config.modelId),
    [config.modelId, discoveredOptions],
  )

  const hasCustomValues = useMemo(() => {
    if (!agentDefaults) return false
    return (
      (config.modelId !== null && config.modelId !== agentDefaults.model) ||
      (config.reasoningEffort !== null &&
        config.reasoningEffort !== agentDefaults.reasoning_effort) ||
      (config.permissionPolicy !== null &&
        config.permissionPolicy !== agentDefaults.permission_policy)
    )
  }, [agentDefaults, config.modelId, config.reasoningEffort, config.permissionPolicy])

  useEffect(() => {
    onChange({
      agentId: config.agentId,
      modelId: config.modelId,
      reasoningEffort: config.reasoningEffort,
      permissionPolicy: config.permissionPolicy,
      selection: config.selection,
      overrides: config.overrides,
    })
  }, [
    config.agentId,
    config.modelId,
    config.overrides,
    config.permissionPolicy,
    config.reasoningEffort,
    config.selection,
    onChange,
  ])

  useEffect(() => {
    if (!discoveredOptions || !config.modelId) return
    if (!discoveredOptions.models.some((m) => m.id === config.modelId)) {
      config.setModelId(null)
    }
  }, [config, discoveredOptions])

  useEffect(() => {
    if (!discoveredOptions || !config.reasoningEffort) return
    if (!reasoningOptionsForModel.some((o) => o.id === config.reasoningEffort)) {
      config.setReasoningEffort(null)
    }
  }, [config, discoveredOptions, reasoningOptionsForModel])

  if (agentsQuery.isLoading) {
    return (
      <div className={cn('space-y-2', className)}>
        <Skeleton className="h-9 w-full" />
        <div className="grid grid-cols-3 gap-2">
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-9 w-full" />
        </div>
      </div>
    )
  }

  const overrideGridCols = showPolicySelector ? 'sm:grid-cols-3' : 'sm:grid-cols-2'
  const showOverrides = showAgentSelector ? config.agentId : config.agentId || initialAgentId

  return (
    <div className={cn('space-y-3', className)}>
      {showAgentSelector && (
        <AgentSelector
          id="execution-config-agent"
          agents={visibleAgents}
          value={config.agentId}
          disabled={disabled || agentsQuery.isError}
          isLoading={agentsQuery.isLoading}
          hasWarning={config.discoveredOptions.isError}
          onChange={(id) => {
            const agent = visibleAgents.find((a) => a.id === id) ?? null
            config.setAgentId(id)
            if (agent) {
              config.setModelId(agent.model)
              config.setReasoningEffort(agent.reasoning_effort)
              config.setPermissionPolicy(agent.permission_policy)
            } else {
              config.setModelId(null)
              config.setReasoningEffort(null)
              config.setPermissionPolicy(null)
            }
          }}
        />
      )}

      {showOverrides ? (
        <>
          {showAgentSelector && <Separator />}
          <div className={cn('grid gap-2', overrideGridCols)}>
            <ModelSelector
              id="execution-config-model"
              models={discoveredOptions?.models ?? []}
              recentModelIds={recentModelIds}
              value={config.modelId}
              disabled={disabled}
              isLoading={config.discoveredOptions.isFetching}
              hasError={config.discoveredOptions.isError}
              onChange={config.setModelId}
            />
            <ReasoningSelector
              id="execution-config-reasoning"
              options={reasoningOptionsForModel}
              value={config.reasoningEffort}
              disabled={disabled}
              isLoading={config.discoveredOptions.isFetching}
              hasError={config.discoveredOptions.isError}
              onChange={config.setReasoningEffort}
            />
            {showPolicySelector && (
              <PolicySelector
                id="execution-config-policy"
                value={config.permissionPolicy}
                disabled={disabled}
                onChange={config.setPermissionPolicy}
              />
            )}
          </div>

          {hasCustomValues ? (
            <div className="flex items-center min-h-[20px]">
              <button
                type="button"
                className="flex items-center gap-1 text-[11px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline cursor-pointer transition-colors duration-150"
                onClick={config.resetToAgentDefaults}
              >
                <ArrowCounterClockwise size={11} />
                Reset to agent defaults
              </button>
            </div>
          ) : null}
        </>
      ) : null}

      {agentsQuery.isError ? (
        <p className="text-xs text-destructive">Could not load agents.</p>
      ) : null}
    </div>
  )
}
