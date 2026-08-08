import { useCallback, useMemo } from 'react'
import { ModelSelector } from '@/components/execution-config/ModelSelector'
import { PolicySelector } from '@/components/execution-config/PolicySelector'
import { ReasoningSelector } from '@/components/execution-config/ReasoningSelector'
import {
  AdvancedJsonField,
  CommandOverridesFields,
  ExecutorConfigFields,
  safeParseConfig,
  setConfigField,
} from '@/components/executor-config-fields'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { getReasoningOptionsForModel, useDiscoveredOptions } from '@/hooks/useDiscoveredOptions'
import { productTerm } from '@/lib/i18n'
import type { Daemon } from '@/types/generated'
import { executorDisplayNames, executorTypes } from './constants'
import type { AgentFormState } from './form-utils'
import { AgentSection, FormField } from './shared'

export function AgentForm({
  form,
  mode,
  agentId,
  daemons,
  showDaemonSelector = true,
  onChange,
}: {
  form: AgentFormState
  mode: 'create' | 'edit'
  agentId?: string
  daemons: Daemon[]
  showDaemonSelector?: boolean
  onChange: (form: AgentFormState) => void
}) {
  const update = (patch: Partial<AgentFormState>) => onChange({ ...form, ...patch })

  const proxyId = agentId ?? null
  const discoveredOptions = useDiscoveredOptions(proxyId, mode === 'create' ? form.executor_type : null)
  const opts = discoveredOptions.data

  const reasoningOptionsForModel = useMemo(
    () => getReasoningOptionsForModel(opts, form.model),
    [form.model, opts],
  )

  const handleExecutorTypeChange = (executorType: string) => {
    update({ executor_type: executorType, model: '', reasoning_effort: '', config_json: '{}' })
  }

  const handleFieldChange = useCallback(
    (key: string, value: unknown) => {
      onChange({ ...form, config_json: setConfigField(form.config_json, key, value) })
    },
    [form, onChange],
  )

  const cfg = useMemo(() => safeParseConfig(form.config_json), [form.config_json])

  return (
    <div className="max-w-[640px]">
      <AgentSection title="Identity" description="Display name and executor type for this agent.">
        <div className="grid gap-3 sm:grid-cols-2">
          <FormField label="Name" htmlFor="agent-name">
            <Input
              id="agent-name"
              autoFocus
              placeholder="My Opus Coder"
              value={form.name}
              onChange={(e) => update({ name: e.target.value })}
            />
          </FormField>
          <FormField label="Executor" htmlFor="agent-executor">
            <Select
              id="agent-executor"
              value={form.executor_type}
              disabled={mode === 'edit'}
              options={executorTypes.map((type) => ({
                value: type,
                label: executorDisplayNames[type],
              }))}
              onChange={(v) => handleExecutorTypeChange(v)}
            />
          </FormField>
        </div>
        <FormField label="Description" htmlFor="agent-description">
          <Input
            id="agent-description"
            placeholder="What does this agent do?"
            value={form.description}
            onChange={(e) => update({ description: e.target.value })}
          />
        </FormField>
      </AgentSection>

      {showDaemonSelector ? (
        <AgentSection
          title={productTerm('runtime')}
          description={`Which ${productTerm('runtime').toLowerCase()} runs this agent's tasks. Leave blank to use any available.`}
        >
          <FormField
            label={`Pinned ${productTerm('runtime').toLowerCase()} (optional)`}
            htmlFor="agent-daemon"
          >
            <Select
              id="agent-daemon"
              value={form.daemon_id}
              placeholder={`Any available ${productTerm('runtime').toLowerCase()}`}
              options={daemons.map((d) => ({
                value: d.id,
                label: (d.hostname || d.machine_id) + (d.status !== 'online' ? ' (offline)' : ''),
                disabled: d.status !== 'online',
              }))}
              onChange={(v) => update({ daemon_id: v })}
            />
            {daemons.length === 0 && (
              <p className="mt-1 text-[11px] text-muted-foreground">
                No {productTerm('runtime', 0).toLowerCase()} registered — agent will use any available
              </p>
            )}
          </FormField>
        </AgentSection>
      ) : null}

      <AgentSection title="Model & behavior" description="Model, reasoning effort, and permission policy for this agent.">
        <div className="grid gap-3 sm:grid-cols-3">
          <ModelSelector
            id="agent-model"
            models={opts?.models ?? []}
            recentModelIds={[]}
            value={form.model || null}
            isLoading={discoveredOptions.isFetching}
            hasError={discoveredOptions.isError}
            onChange={(modelId) => update({ model: modelId ?? '', reasoning_effort: '' })}
          />
          <ReasoningSelector
            id="agent-reasoning"
            options={reasoningOptionsForModel}
            value={form.reasoning_effort || null}
            isLoading={discoveredOptions.isFetching}
            hasError={discoveredOptions.isError}
            onChange={(effort) => update({ reasoning_effort: effort ?? '' })}
          />
          <PolicySelector
            id="agent-policy"
            value={form.permission_policy || null}
            onChange={(policy) => update({ permission_policy: policy ?? '' })}
          />
        </div>
      </AgentSection>

      <AgentSection title="System prompt" description="Injected as additional context when this agent is dispatched.">
        <Textarea
          id="agent-prompt"
          rows={6}
          placeholder="You are a specialized coding agent focused on..."
          value={form.prompt_template}
          onChange={(e) => update({ prompt_template: e.target.value })}
        />
      </AgentSection>

      <AgentSection
        title={`${executorDisplayNames[form.executor_type] ?? form.executor_type} settings`}
        description="Executor-specific configuration, command overrides, and raw JSON."
      >
        <div className="space-y-3">
          <ExecutorConfigFields executorType={form.executor_type} cfg={cfg} onChange={handleFieldChange} />
          <CommandOverridesFields cfg={cfg} onChange={handleFieldChange} />
          <AdvancedJsonField
            value={form.config_json}
            onChange={(json) => update({ config_json: json })}
            rows={6}
            defaultOpen={false}
          />
        </div>
      </AgentSection>

      <AgentSection title="Capacity" description="Concurrency limit and capability tags for task routing.">
        <div className="grid gap-3 sm:grid-cols-2">
          <FormField label="Max concurrent tasks" htmlFor="agent-max">
            <Input
              id="agent-max"
              type="number"
              min={1}
              value={form.max_concurrent_tasks}
              onChange={(e) => update({ max_concurrent_tasks: e.target.value })}
            />
          </FormField>
          <FormField label="Capabilities" htmlFor="agent-caps">
            <Input
              id="agent-caps"
              placeholder="rust, typescript"
              value={form.capabilities}
              onChange={(e) => update({ capabilities: e.target.value })}
            />
            <p className="mt-1 text-[11px] text-muted-foreground">Comma-separated tags</p>
          </FormField>
        </div>
      </AgentSection>
    </div>
  )
}
