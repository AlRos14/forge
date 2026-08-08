import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { ApiError } from '@/api/client'
import {
  useUpdateWorkflow,
  useWorkflowPromptBuildersQuery,
  useWorkflowQuery,
  useWorkflowTemplatesQuery,
} from '@/api/hooks'
import { settingsErrorMessage } from '@/components/settings/project-settings-utils'
import { SettingsSection } from '@/components/settings/SettingsSection'
import { WorkflowConfigurationSection } from '@/components/settings/workflow/WorkflowConfigurationSection'
import { WorkflowDispatchSection } from '@/components/settings/workflow/WorkflowDispatchSection'
import { WorkflowJsonEditorSection } from '@/components/settings/workflow/WorkflowJsonEditorSection'
import {
  cloneWorkflow,
  configurationFields,
  groupStatesByColumn,
  parseWorkflowFieldValue,
  setWorkflowFieldValue,
  stateRecords,
  workflowFieldValue,
} from '@/components/settings/workflow/workflow-utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { useLayoutStore } from '@/stores/layout'
import { productTerm } from '@/lib/i18n'
import type { StateDefinition, WorkflowDefinition } from '@/types/generated'

export function WorkflowTab({
  projectId,
  workflowTemplateName,
}: {
  projectId: string
  workflowTemplateName: string | undefined
}) {
  const workflowQuery = useWorkflowQuery(projectId)
  const promptBuildersQuery = useWorkflowPromptBuildersQuery()
  const templatesQuery = useWorkflowTemplatesQuery()
  const updateWorkflow = useUpdateWorkflow()
  const theme = useLayoutStore((s) => s.theme)
  const [workflowJson, setWorkflowJson] = useState('')
  const [selectedTemplate, setSelectedTemplate] = useState('')
  const [editorOpen, setEditorOpen] = useState(false)
  const [configDraft, setConfigDraft] = useState<Record<string, string>>({})
  const [dispatchDraft, setDispatchDraft] = useState<WorkflowDefinition | null>(null)
  const [activeDispatchColumn, setActiveDispatchColumn] = useState<string>('')
  const [activeDispatchState, setActiveDispatchState] = useState<string>('')

  const fields = configurationFields(workflowQuery.data)

  useEffect(() => {
    if (!workflowQuery.data) return
    const timeout = window.setTimeout(() => {
      setWorkflowJson(JSON.stringify(workflowQuery.data, null, 2))
      setConfigDraft(
        Object.fromEntries(
          configurationFields(workflowQuery.data).map((field) => [
            field.id,
            workflowFieldValue(workflowQuery.data, field),
          ]),
        ),
      )
      setDispatchDraft(cloneWorkflow(workflowQuery.data))
      const firstState = workflowQuery.data.states[0]
      if (firstState) {
        setActiveDispatchColumn(firstState.column)
        setActiveDispatchState(firstState.name)
      }
    }, 0)
    return () => window.clearTimeout(timeout)
  }, [workflowQuery.data])

  const applyTemplate = () => {
    if (!selectedTemplate || updateWorkflow.isPending) return
    updateWorkflow.mutate(
      { projectId, body: { template_name: selectedTemplate, definition: null } },
      {
        onSuccess: () => {
          toast.success(`Template "${selectedTemplate}" applied`)
          setSelectedTemplate('')
        },
        onError: (error) => {
          if (error instanceof ApiError && error.status === 409) {
            toast.error(
              `Cannot apply template: tasks exist in ${productTerm('phase', 0).toLowerCase()} that would be removed`,
            )
            return
          }
          toast.error(settingsErrorMessage(error, 'Template apply failed'))
        },
      },
    )
  }

  const saveWorkflow = (onSuccess?: () => void) => {
    if (updateWorkflow.isPending) return
    let parsed: unknown
    try {
      parsed = JSON.parse(workflowJson)
    } catch {
          toast.error('Workflow is not valid JSON')
      return
    }
    updateWorkflow.mutate(
      {
        projectId,
        body: {
          template_name: null,
          definition: parsed as Parameters<typeof updateWorkflow.mutate>[0]['body']['definition'],
        },
      },
      {
        onSuccess: () => {
          toast.success('Workflow saved')
          onSuccess?.()
        },
        onError: (error) => {
          if (error instanceof ApiError && error.status === 409) {
            toast.error(
              `Cannot update workflow: tasks exist in ${productTerm('phase', 0).toLowerCase()} that would be removed`,
            )
            return
          }
          toast.error(settingsErrorMessage(error, 'Workflow update failed'))
        },
      },
    )
  }

  const saveWorkflowConfiguration = () => {
    if (!workflowQuery.data || updateWorkflow.isPending) return
    const nextWorkflow = cloneWorkflow(workflowQuery.data)

    for (const field of fields) {
      let value: unknown
      try {
        value = parseWorkflowFieldValue(field, configDraft[field.id] ?? '')
      } catch (error) {
        toast.error(error instanceof Error ? error.message : 'Configuration value is invalid')
        return
      }
      if (!setWorkflowFieldValue(nextWorkflow, field, value)) {
        toast.error(`Cannot apply "${field.label}" to this workflow definition`)
        return
      }
    }

    updateWorkflow.mutate(
      { projectId, body: { template_name: null, definition: nextWorkflow } },
      {
        onSuccess: () => {
          setWorkflowJson(JSON.stringify(nextWorkflow, null, 2))
          setConfigDraft(
            Object.fromEntries(
              configurationFields(nextWorkflow).map((field) => [
                field.id,
                workflowFieldValue(nextWorkflow, field),
              ]),
            ),
          )
          toast.success('Workflow configuration saved')
        },
        onError: (error) => {
          if (error instanceof ApiError && error.status === 409) {
            toast.error(
              `Cannot update workflow: tasks exist in ${productTerm('phase', 0).toLowerCase()} that would be removed`,
            )
            return
          }
          toast.error(settingsErrorMessage(error, 'Workflow configuration update failed'))
        },
      },
    )
  }

  const updateDispatchDraft = (
    updater: (nextWorkflow: WorkflowDefinition, states: StateDefinition[]) => void,
  ) => {
    setDispatchDraft((current) => {
      const baseline = current ?? (workflowQuery.data ? cloneWorkflow(workflowQuery.data) : null)
      if (!baseline) return baseline
      const nextWorkflow = cloneWorkflow(baseline)
      updater(nextWorkflow, stateRecords(nextWorkflow))
      setWorkflowJson(JSON.stringify(nextWorkflow, null, 2))
      return nextWorkflow
    })
  }

  const builderOptions = (promptBuildersQuery.data ?? []).map((entry) => ({
    value: entry.id,
    label: entry.label,
  }))
  const executionPolicyOptions = [
    { value: '', label: 'Default policy' },
    { value: 'new_execution', label: `New ${productTerm('run').toLowerCase()}` },
    { value: 'resume_latest_target_role_thread', label: 'Resume latest target-role thread' },
  ]
  const dispatchStates = stateRecords(dispatchDraft ?? workflowQuery.data)
  const dispatchColumnGroups = groupStatesByColumn(dispatchStates)

  return (
    <>
      <div className="mb-8 flex items-start justify-between">
        <div>
          <h2 className="text-page font-semibold tracking-tight">Workflow definition</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            The {productTerm('phase', 0).toLowerCase()} every task in this project moves through. Roles defined here surface
            in the agent register flow.
          </p>
        </div>
        {workflowTemplateName && (
          <Badge variant="secondary" className="mt-1 shrink-0 font-normal">
            Based on: {workflowTemplateName}
          </Badge>
        )}
      </div>

      {workflowQuery.isLoading || templatesQuery.isLoading ? (
        <Skeleton className="h-48 w-full" />
      ) : (
        <>
          <WorkflowConfigurationSection
            fields={fields}
            configDraft={configDraft}
            setConfigDraft={setConfigDraft}
            isSaving={updateWorkflow.isPending}
            onSave={saveWorkflowConfiguration}
          />

          <WorkflowDispatchSection
            dispatchDraft={dispatchDraft}
            dispatchColumnGroups={dispatchColumnGroups}
            activeDispatchColumn={activeDispatchColumn}
            activeDispatchState={activeDispatchState}
            setActiveDispatchColumn={setActiveDispatchColumn}
            setActiveDispatchState={setActiveDispatchState}
            builderOptions={builderOptions}
            executionPolicyOptions={executionPolicyOptions}
            isSaving={updateWorkflow.isPending}
            onSave={() =>
              updateWorkflow.mutate(
                { projectId, body: { template_name: null, definition: dispatchDraft } },
                {
                  onSuccess: () => toast.success('Dispatch settings saved'),
                  onError: (error) =>
                    toast.error(settingsErrorMessage(error, 'Dispatch update failed')),
                },
              )
            }
            updateDispatchDraft={updateDispatchDraft}
          />

          <SettingsSection
            title="Apply template"
            description="Snapshots the selected template into this project's workflow definition. Existing per-project edits are overwritten."
          >
            <div className="flex gap-2">
              <Select
                className="flex-1"
                value={selectedTemplate}
                placeholder="Select a template…"
                options={(templatesQuery.data ?? []).map((tpl) => ({
                  value: tpl.name,
                  label: tpl.display_name + (tpl.description ? ` — ${tpl.description}` : ''),
                }))}
                onChange={(v) => setSelectedTemplate(v)}
              />
              <Button
                variant="secondary"
                disabled={!selectedTemplate || updateWorkflow.isPending}
                onClick={applyTemplate}
              >
                Apply
              </Button>
            </div>
          </SettingsSection>

          <WorkflowJsonEditorSection
            workflowJson={workflowJson}
            setWorkflowJson={setWorkflowJson}
            editorOpen={editorOpen}
            setEditorOpen={setEditorOpen}
            theme={theme}
            isSaving={updateWorkflow.isPending}
            onSave={() => saveWorkflow(() => setEditorOpen(false))}
          />
        </>
      )}
    </>
  )
}
