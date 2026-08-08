import { useState, type Dispatch, type SetStateAction } from 'react'
import { CaretDown } from '@phosphor-icons/react'
import { cn } from '@/lib/cn'
import { SettingsSection } from '@/components/settings/SettingsSection'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { productTerm } from '@/lib/i18n'
import type { StateDefinition, WorkflowDefinition } from '@/types/generated'
import {
  type ColumnGroup,
  mutateDispatchField,
  mutateDispatchInstructions,
  readDispatchBuilder,
  readDispatchExecutionPolicy,
  readDispatchInstructions,
  triggerRecords,
} from './workflow-utils'

export function WorkflowDispatchSection({
  dispatchDraft,
  dispatchColumnGroups,
  activeDispatchColumn,
  activeDispatchState,
  setActiveDispatchColumn,
  setActiveDispatchState,
  builderOptions,
  executionPolicyOptions,
  isSaving,
  onSave,
  updateDispatchDraft,
}: {
  dispatchDraft: WorkflowDefinition | null
  dispatchColumnGroups: ColumnGroup[]
  activeDispatchColumn: string
  activeDispatchState: string
  setActiveDispatchColumn: Dispatch<SetStateAction<string>>
  setActiveDispatchState: Dispatch<SetStateAction<string>>
  builderOptions: Array<{ value: string; label: string }>
  executionPolicyOptions: Array<{ value: string; label: string }>
  isSaving: boolean
  onSave: () => void
  updateDispatchDraft: (updater: (nextWorkflow: WorkflowDefinition, states: StateDefinition[]) => void) => void
}) {
  const [openTriggers, setOpenTriggers] = useState<Record<string, boolean>>({})

  return (
    <SettingsSection
      title="Dispatch"
      description={`Configure ${productTerm('phase').toLowerCase()} and trigger dispatch intent. Trigger dispatch uses the target ${productTerm('phase').toLowerCase()}'s role automatically.`}
    >
      {dispatchColumnGroups.length === 0 ? (
        <p className="rounded-md border px-3 py-2 text-xs text-muted-foreground">
          This workflow does not define any {productTerm('phase', 0).toLowerCase()}.
        </p>
      ) : (
        <div className="rounded-md border">
          <div className="flex items-center justify-between border-b">
            <div className="flex">
              {dispatchColumnGroups.map((group) => (
                <button
                  key={group.column}
                  type="button"
                  className={cn(
                    '-mb-px border-b-2 px-3 py-2.5 text-sm font-medium transition-colors',
                    activeDispatchColumn === group.column
                      ? 'border-primary text-foreground'
                      : 'border-transparent text-muted-foreground hover:text-foreground',
                  )}
                  onClick={() => {
                    setActiveDispatchColumn(group.column)
                    setActiveDispatchState(group.states[0]?.name ?? '')
                  }}
                >
                  {group.displayName}
                </button>
              ))}
            </div>
            <div className="px-2">
              <Button size="sm" disabled={isSaving || !dispatchDraft} onClick={onSave}>
                {isSaving ? 'Saving...' : 'Save'}
              </Button>
            </div>
          </div>

          {(() => {
            const activeGroup = dispatchColumnGroups.find((g) => g.column === activeDispatchColumn)
            if (!activeGroup) return null
            const activeStateDef =
              activeGroup.states.find((s) => s.name === activeDispatchState) ?? activeGroup.states[0]
            if (!activeStateDef) return null
            const multiState = activeGroup.states.length > 1
            const activeTriggers = triggerRecords(activeStateDef)
            const stateOptions = dispatchColumnGroups.flatMap((g) =>
              g.states.map((s) => ({ value: s.name, label: s.display_name || s.name })),
            )

            return (
              <div className={cn('flex min-h-[320px]', multiState && 'divide-x')}>
                {multiState && (
                  <div className="w-36 shrink-0 py-2">
                    {activeGroup.states.map((state) => (
                      <button
                        key={state.name}
                        type="button"
                        className={cn(
                          'w-full px-3 py-1.5 text-left text-sm transition-colors hover:bg-muted/50',
                          activeDispatchState === state.name
                            ? 'bg-muted font-medium text-foreground'
                            : 'text-muted-foreground',
                        )}
                        onClick={() => setActiveDispatchState(state.name)}
                      >
                        {state.display_name}
                      </button>
                    ))}
                  </div>
                )}
                <div className="flex-1 space-y-3 p-4">
                  <div className="grid gap-3 md:grid-cols-2">
                    <div className="space-y-1.5">
                      <Label>Prompt builder</Label>
                      <Select
                        aria-label="Select prompt builder"
                        value={readDispatchBuilder(activeStateDef)}
                        placeholder="Select prompt builder…"
                        options={builderOptions}
                        onChange={(value) =>
                          updateDispatchDraft((_nextWorkflow, states) => {
                            const nextState = states.find((s) => s.name === activeStateDef.name)
                            if (!nextState) return
                            mutateDispatchField(nextState, 'builder', value)
                          })
                        }
                      />
                    </div>
                    <div className="space-y-1.5">
                      <Label>{productTerm('run')} policy</Label>
                      <Select
                        aria-label={`Select ${productTerm('run').toLowerCase()} policy`}
                        value={readDispatchExecutionPolicy(activeStateDef)}
                        placeholder="Default policy"
                        options={executionPolicyOptions}
                        onChange={(value) =>
                          updateDispatchDraft((_nextWorkflow, states) => {
                            const nextState = states.find((s) => s.name === activeStateDef.name)
                            if (!nextState) return
                            mutateDispatchField(nextState, 'execution_policy', value)
                          })
                        }
                      />
                    </div>
                  </div>
                  <div className="space-y-1.5">
                    <Label>Prompt instructions</Label>
                    <Textarea
                      rows={6}
                      value={readDispatchInstructions(activeStateDef)}
                      onChange={(event) =>
                        updateDispatchDraft((_nextWorkflow, states) => {
                          const nextState = states.find((s) => s.name === activeStateDef.name)
                          if (!nextState) return
                          mutateDispatchInstructions(nextState, event.target.value)
                        })
                      }
                    />
                  </div>
                  {activeTriggers.length > 0 && (
                    <CollapsibleSection
                      title="Trigger dispatch"
                      badge={activeTriggers.length}
                      className="border-t pt-3"
                    >
                      <div className="space-y-2">
                        {activeTriggers.map(({ name: triggerName, trigger }) => {
                          const isOpen = openTriggers[triggerName] ?? false
                          const targetState = typeof trigger.to === 'string' ? trigger.to : ''
                          const targetLabel =
                            stateOptions.find((o) => o.value === targetState)?.label ?? targetState
                          return (
                            <div key={triggerName} className="rounded-md border">
                              <button
                                type="button"
                                className="flex w-full items-center justify-between px-3 py-2.5 text-left transition-colors hover:bg-muted/40"
                                onClick={() =>
                                  setOpenTriggers((prev) => ({
                                    ...prev,
                                    [triggerName]: !isOpen,
                                  }))
                                }
                              >
                                <span className="flex items-center gap-2 text-sm">
                                  <span className="font-mono font-medium text-foreground">
                                    {triggerName}
                                  </span>
                                  {targetLabel && (
                                    <>
                                      <span className="text-muted-foreground">→</span>
                                      <span className="text-muted-foreground">{targetLabel}</span>
                                    </>
                                  )}
                                </span>
                                <CaretDown
                                  size={13}
                                  className={cn(
                                    'shrink-0 text-muted-foreground transition-transform',
                                    isOpen && 'rotate-180',
                                  )}
                                />
                              </button>
                              {isOpen && (
                                <div className="space-y-3 border-t p-3">
                                  <div className="space-y-1.5">
                                    <Label>Target {productTerm('phase')}</Label>
                                    <Select
                                      aria-label={`Select target ${productTerm('phase').toLowerCase()}`}
                                      value={targetState}
                                      placeholder={`Select target ${productTerm('phase').toLowerCase()}…`}
                                      options={stateOptions}
                                      onChange={(value) =>
                                        updateDispatchDraft((_nextWorkflow, states) => {
                                          const nextState = states.find(
                                            (s) => s.name === activeStateDef.name,
                                          )
                                          if (!nextState) return
                                          const nextTrigger = triggerRecords(nextState).find(
                                            (c) => c.name === triggerName,
                                          )?.trigger
                                          if (!nextTrigger) return
                                          nextTrigger.to = value
                                        })
                                      }
                                    />
                                  </div>
                                  <div className="grid gap-3 md:grid-cols-2">
                                    <div className="space-y-1.5">
                                      <Label>Prompt builder</Label>
                                      <Select
                                        aria-label="Select prompt builder"
                                        value={readDispatchBuilder(trigger)}
                                        placeholder="Select prompt builder…"
                                        options={builderOptions}
                                        onChange={(value) =>
                                          updateDispatchDraft((_nextWorkflow, states) => {
                                            const nextState = states.find(
                                              (s) => s.name === activeStateDef.name,
                                            )
                                            if (!nextState) return
                                            const nextTrigger = triggerRecords(nextState).find(
                                              (c) => c.name === triggerName,
                                            )?.trigger
                                            if (!nextTrigger) return
                                            mutateDispatchField(nextTrigger, 'builder', value)
                                          })
                                        }
                                      />
                                    </div>
                                    <div className="space-y-1.5">
                                      <Label>{productTerm('run')} policy</Label>
                                      <Select
                                        aria-label={`Select ${productTerm('run').toLowerCase()} policy`}
                                        value={readDispatchExecutionPolicy(trigger)}
                                        placeholder="Default policy"
                                        options={executionPolicyOptions}
                                        onChange={(value) =>
                                          updateDispatchDraft((_nextWorkflow, states) => {
                                            const nextState = states.find(
                                              (s) => s.name === activeStateDef.name,
                                            )
                                            if (!nextState) return
                                            const nextTrigger = triggerRecords(nextState).find(
                                              (c) => c.name === triggerName,
                                            )?.trigger
                                            if (!nextTrigger) return
                                            mutateDispatchField(
                                              nextTrigger,
                                              'execution_policy',
                                              value,
                                            )
                                          })
                                        }
                                      />
                                    </div>
                                  </div>
                                  <div className="space-y-1.5">
                                    <Label>Prompt instructions</Label>
                                    <Textarea
                                      rows={4}
                                      value={readDispatchInstructions(trigger)}
                                      onChange={(event) =>
                                        updateDispatchDraft((_nextWorkflow, states) => {
                                          const nextState = states.find(
                                            (s) => s.name === activeStateDef.name,
                                          )
                                          if (!nextState) return
                                          const nextTrigger = triggerRecords(nextState).find(
                                            (c) => c.name === triggerName,
                                          )?.trigger
                                          if (!nextTrigger) return
                                          mutateDispatchInstructions(nextTrigger, event.target.value)
                                        })
                                      }
                                    />
                                  </div>
                                </div>
                              )}
                            </div>
                          )
                        })}
                      </div>
                    </CollapsibleSection>
                  )}
                </div>
              </div>
            )
          })()}
        </div>
      )}
    </SettingsSection>
  )
}
