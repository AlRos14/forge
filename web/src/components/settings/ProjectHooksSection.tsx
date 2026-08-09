import { useMemo, useState, type FormEvent } from 'react'
import { Link } from '@tanstack/react-router'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  CaretRight,
  DotsThreeVertical,
  PencilSimple,
  Plus,
  Trash,
} from '@phosphor-icons/react'
import { toast } from 'sonner'
import { ApiError, apiFetch } from '@/api/client'
import { useAgentsQuery, useProjectHookRunsQuery } from '@/api/hooks'
import { qk } from '@/api/query-keys'
import { ErrorBanner } from '@/components/error-banner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { ComboSelect, type ComboSelectOption } from '@/components/ui/combo-select'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import { cn } from '@/lib/cn'
import { productTerm } from '@/lib/i18n'
import type { Project, TaskType } from '@/types/generated'
import type { ProjectHookAction } from '@/types/generated/bindings/ProjectHookAction'
import type { ProjectHookRule } from '@/types/generated/bindings/ProjectHookRule'
import type { ProjectHookRunResponse } from '@/types/generated/bindings/ProjectHookRunResponse'
import type { ProjectHookRunStatus } from '@/types/generated/bindings/ProjectHookRunStatus'

type ProjectHookActionType = ProjectHookAction['type']
type ProjectHookTriggerType = ProjectHookRule['trigger']['type']
type PriorityOption = '' | 'low' | 'medium' | 'high' | 'critical'
type SeverityOption = '' | 'info' | 'warning' | 'error'

type ProjectHookRuleFormState = {
  id: string
  name: string
  enabled: boolean
  triggerType: ProjectHookTriggerType
  actionType: ProjectHookActionType
  dispatchAgentId: string
  dispatchPrompt: string
  dispatchFollowUp: unknown
  createTaskTitle: string
  createTaskDescription: string
  createTaskPriority: PriorityOption
  createTaskType: TaskType | null
  addCommentTargetTaskId: string
  addCommentContent: string
  notifyTitle: string
  notifyMessage: string
  notifySeverity: SeverityOption
  cooldownSeconds: string
  maxConcurrentRuns: string
  filters: unknown
}

type ProjectHookActionWire =
  | { type: 'dispatch_agent'; agent_id: string; prompt: string | null; follow_up: unknown }
  | {
      type: 'create_task'
      title: string
      description: string | null
      task_type: TaskType | null
      priority: number | null
    }
  | { type: 'add_comment'; target_task_id: string | null; content: string }
  | { type: 'notify'; title: string; message: string; severity: string | null }

type ProjectHookRuleWire = {
  id: string
  enabled: boolean
  name: string
  trigger: { type: ProjectHookTriggerType }
  filters: unknown
  action: ProjectHookActionWire
  cooldown_seconds: number | null
  max_concurrent_runs: number
}

type DialogMode = { type: 'add' } | { type: 'edit'; index: number }

const ACTION_OPTIONS: Array<{ value: ProjectHookActionType; label: string }> = [
  { value: 'dispatch_agent', label: 'dispatch_agent' },
  { value: 'create_task', label: 'create_task' },
  { value: 'add_comment', label: 'add_comment' },
  { value: 'notify', label: 'notify' },
]

const TRIGGER_OPTIONS: Array<{ value: ProjectHookTriggerType; label: string }> = [
  { value: 'project.all_work_completed', label: 'project.all_work_completed' },
]

const PRIORITY_OPTIONS: Array<{ value: PriorityOption; label: string }> = [
  { value: '', label: 'None' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'critical', label: 'Critical' },
]

const SEVERITY_OPTIONS: Array<{ value: SeverityOption; label: string }> = [
  { value: '', label: 'None' },
  { value: 'info', label: 'Info' },
  { value: 'warning', label: 'Warning' },
  { value: 'error', label: 'Error' },
]

const PRIORITY_VALUES: Record<Exclude<PriorityOption, ''>, number> = {
  low: 0,
  medium: 1,
  high: 2,
  critical: 3,
}

const RUN_STATUS_CLASSES: Record<ProjectHookRunStatus, string> = {
  queued: 'border-muted-foreground/30 bg-muted text-muted-foreground',
  running: 'border-blue-500/30 bg-blue-500/10 text-blue-300',
  dispatched: 'border-violet-500/30 bg-violet-500/10 text-violet-300',
  skipped: 'border-amber-500/30 bg-amber-500/10 text-amber-300',
  failed: 'border-red-500/30 bg-red-500/10 text-red-300',
  completed: 'border-emerald-500/30 bg-emerald-500/10 text-emerald-300',
}

const RUN_STATUS_DOT: Record<ProjectHookRunStatus, string> = {
  queued: 'bg-muted-foreground/60',
  running: 'bg-blue-400',
  dispatched: 'bg-violet-400',
  skipped: 'bg-amber-400',
  failed: 'bg-red-400',
  completed: 'bg-emerald-400',
}

function emptyRuleForm(): ProjectHookRuleFormState {
  return {
    id: '',
    name: '',
    enabled: false,
    triggerType: 'project.all_work_completed',
    actionType: 'dispatch_agent',
    dispatchAgentId: '',
    dispatchPrompt: '',
    dispatchFollowUp: null,
    createTaskTitle: '',
    createTaskDescription: '',
    createTaskPriority: '',
    createTaskType: null,
    addCommentTargetTaskId: '',
    addCommentContent: '',
    notifyTitle: '',
    notifyMessage: '',
    notifySeverity: '',
    cooldownSeconds: '',
    maxConcurrentRuns: '1',
    filters: null,
  }
}

function compactId(value: string | null | undefined): string {
  if (!value) return '-'
  return value.length > 12 ? `${value.slice(0, 8)}...` : value
}

function formatRelativeTime(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  const diffMs = date.getTime() - Date.now()
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })
  const seconds = Math.round(diffMs / 1000)
  if (Math.abs(seconds) < 60) return rtf.format(seconds, 'second')
  const minutes = Math.round(seconds / 60)
  if (Math.abs(minutes) < 60) return rtf.format(minutes, 'minute')
  const hours = Math.round(minutes / 60)
  if (Math.abs(hours) < 24) return rtf.format(hours, 'hour')
  return rtf.format(Math.round(hours / 24), 'day')
}

function formatDate(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString()
}

function formatCooldown(seconds: bigint | number | null | undefined): string {
  const parsed = numberOrNull(seconds)
  if (parsed === null || parsed === 0) return 'no cooldown'
  if (parsed < 60) return `${parsed}s`
  if (parsed < 3600) return `${Math.round(parsed / 60)}m`
  if (parsed < 86400) return `${Math.round(parsed / 3600)}h`
  return `${Math.round(parsed / 86400)}d`
}

function shortTrigger(triggerType: string): string {
  const last = triggerType.split('.').pop() ?? triggerType
  return last.replace(/_/g, ' ')
}

function actionSummary(
  action: ProjectHookRule['action'],
  agentNames?: Map<string, string>,
): string {
  switch (action.type) {
    case 'dispatch_agent': {
      const prompt = action.prompt?.trim()
      const suffix = prompt
        ? ` with prompt "${prompt.length > 60 ? `${prompt.slice(0, 57)}...` : prompt}"`
        : ''
      const agentLabel = agentNames?.get(action.agent_id) ?? action.agent_id
      return `Dispatch ${agentLabel}${suffix}`
    }
    case 'create_task':
      return `Create task "${action.title}"`
    case 'add_comment': {
      const target = action.target_task_id ? compactId(action.target_task_id) : 'source task'
      return `Comment on ${target}`
    }
    case 'notify':
      return `Notify: ${action.title}`
  }
}

function numberOrNull(value: bigint | number | null | undefined): number | null {
  if (value === null || value === undefined) return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

function priorityOptionFromValue(value: bigint | number | null | undefined): PriorityOption {
  const parsed = numberOrNull(value)
  if (parsed === 0) return 'low'
  if (parsed === 1) return 'medium'
  if (parsed === 2) return 'high'
  if (parsed === 3) return 'critical'
  return ''
}

function isActionType(value: string): value is ProjectHookActionType {
  return ACTION_OPTIONS.some((option) => option.value === value)
}

function isPriorityOption(value: string): value is PriorityOption {
  return PRIORITY_OPTIONS.some((option) => option.value === value)
}

function isSeverityOption(value: string): value is SeverityOption {
  return SEVERITY_OPTIONS.some((option) => option.value === value)
}

function ruleToWire(rule: ProjectHookRule): ProjectHookRuleWire {
  const base = {
    id: rule.id,
    enabled: rule.enabled,
    name: rule.name,
    trigger: rule.trigger,
    filters: rule.filters ?? null,
    cooldown_seconds: numberOrNull(rule.cooldown_seconds),
    max_concurrent_runs: rule.max_concurrent_runs,
  }

  switch (rule.action.type) {
    case 'dispatch_agent':
      return {
        ...base,
        action: {
          type: 'dispatch_agent',
          agent_id: rule.action.agent_id,
          prompt: rule.action.prompt,
          follow_up: rule.action.follow_up ?? null,
        },
      }
    case 'create_task':
      return {
        ...base,
        action: {
          type: 'create_task',
          title: rule.action.title,
          description: rule.action.description,
          task_type: rule.action.task_type,
          priority: numberOrNull(rule.action.priority),
        },
      }
    case 'add_comment':
      return {
        ...base,
        action: {
          type: 'add_comment',
          target_task_id: rule.action.target_task_id,
          content: rule.action.content,
        },
      }
    case 'notify':
      return {
        ...base,
        action: {
          type: 'notify',
          title: rule.action.title,
          message: rule.action.message,
          severity: rule.action.severity,
        },
      }
  }
}

function formFromRule(rule: ProjectHookRule): ProjectHookRuleFormState {
  const form = {
    ...emptyRuleForm(),
    id: rule.id,
    name: rule.name,
    enabled: rule.enabled,
    triggerType: rule.trigger.type,
    cooldownSeconds: rule.cooldown_seconds == null ? '' : String(rule.cooldown_seconds),
    maxConcurrentRuns: String(rule.max_concurrent_runs),
    filters: rule.filters ?? null,
  }

  switch (rule.action.type) {
    case 'dispatch_agent':
      return {
        ...form,
        actionType: 'dispatch_agent',
        dispatchAgentId: rule.action.agent_id,
        dispatchPrompt: rule.action.prompt ?? '',
        dispatchFollowUp: rule.action.follow_up ?? null,
      }
    case 'create_task':
      return {
        ...form,
        actionType: 'create_task',
        createTaskTitle: rule.action.title,
        createTaskDescription: rule.action.description ?? '',
        createTaskPriority: priorityOptionFromValue(rule.action.priority),
        createTaskType: rule.action.task_type,
      }
    case 'add_comment':
      return {
        ...form,
        actionType: 'add_comment',
        addCommentTargetTaskId: rule.action.target_task_id ?? '',
        addCommentContent: rule.action.content,
      }
    case 'notify': {
      const severityRaw = rule.action.severity ?? ''
      const notifySeverity: SeverityOption = isSeverityOption(severityRaw) ? severityRaw : ''

      return {
        ...form,
        actionType: 'notify',
        notifyTitle: rule.action.title,
        notifyMessage: rule.action.message,
        notifySeverity,
      }
    }
  }
}

function buildRule(form: ProjectHookRuleFormState): ProjectHookRuleWire | string {
  const id = form.id.trim()
  if (!id) return 'Project hook rule id is required'

  const name = form.name.trim()
  if (!name) return 'Project hook rule name is required'

  const cooldownInput = form.cooldownSeconds.trim()
  const cooldownSeconds =
    cooldownInput === '' ? null : Number.parseInt(cooldownInput, 10)
  if (
    cooldownSeconds !== null &&
    (!Number.isInteger(cooldownSeconds) || cooldownSeconds < 0)
  ) {
    return 'Cooldown seconds must be 0 or greater'
  }

  const maxConcurrentInput = form.maxConcurrentRuns.trim() || '1'
  const maxConcurrentRuns = Number.parseInt(maxConcurrentInput, 10)
  if (!Number.isInteger(maxConcurrentRuns) || maxConcurrentRuns < 1 || maxConcurrentRuns > 10) {
    return 'Max concurrent runs must be between 1 and 10'
  }

  let action: ProjectHookActionWire
  switch (form.actionType) {
    case 'dispatch_agent': {
      const agentId = form.dispatchAgentId.trim()
      if (!agentId) return 'dispatch_agent.agent_id is required'
      action = {
        type: 'dispatch_agent',
        agent_id: agentId,
        prompt: form.dispatchPrompt.trim() || null,
        follow_up: form.dispatchFollowUp ?? null,
      }
      break
    }
    case 'create_task': {
      const title = form.createTaskTitle.trim()
      if (!title) return 'create_task.title is required'
      action = {
        type: 'create_task',
        title,
        description: form.createTaskDescription.trim() || null,
        task_type: form.createTaskType,
        priority: form.createTaskPriority ? PRIORITY_VALUES[form.createTaskPriority] : null,
      }
      break
    }
    case 'add_comment': {
      const content = form.addCommentContent.trim()
      if (!content) return 'add_comment.content is required'
      action = {
        type: 'add_comment',
        target_task_id: form.addCommentTargetTaskId.trim() || null,
        content,
      }
      break
    }
    case 'notify': {
      const title = form.notifyTitle.trim()
      const message = form.notifyMessage.trim()
      if (!title) return 'notify.title is required'
      if (!message) return 'notify.message is required'
      action = {
        type: 'notify',
        title,
        message,
        severity: form.notifySeverity || null,
      }
      break
    }
  }

  return {
    id,
    enabled: form.enabled,
    name,
    trigger: { type: form.triggerType },
    filters: form.filters ?? null,
    action,
    cooldown_seconds: cooldownSeconds,
    max_concurrent_runs: maxConcurrentRuns,
  }
}

function serverErrorMessage(error: unknown, fallback = 'Project hook update failed'): string {
  if (error instanceof ApiError) {
    if (error.status === 400) {
      try {
        const parsed = JSON.parse(error.message) as { message?: unknown }
        if (typeof parsed.message === 'string' && parsed.message) return parsed.message
      } catch {
        return error.message || fallback
      }
      return error.message || fallback
    }
    return getApiErrorMessage(error, fallback)
  }
  if (error instanceof Error) return error.message
  return fallback
}

type RuleRunStats = {
  lastRun: ProjectHookRunResponse | null
  total: number
  completed: number
  failed: number
}

function computeRunStats(
  runs: ProjectHookRunResponse[],
): Map<string, RuleRunStats> {
  const stats = new Map<string, RuleRunStats>()
  for (const run of runs) {
    const existing = stats.get(run.rule_id) ?? {
      lastRun: null,
      total: 0,
      completed: 0,
      failed: 0,
    }
    existing.total += 1
    if (run.status === 'completed') existing.completed += 1
    if (run.status === 'failed') existing.failed += 1
    if (!existing.lastRun) existing.lastRun = run
    stats.set(run.rule_id, existing)
  }
  return stats
}

function TaskLink({ taskId }: { taskId: string | null }) {
  if (!taskId) return <span className="text-muted-foreground">-</span>
  return (
    <Link
      to="/tasks/$taskId"
      params={{ taskId }}
      className="font-mono text-xs text-primary hover:underline"
      title={taskId}
    >
      {compactId(taskId)}
    </Link>
  )
}

function ExecutionLink({ run }: { run: ProjectHookRunResponse }) {
  if (!run.execution_id) return <span className="text-muted-foreground">-</span>
  const taskId = run.automation_task_id ?? run.source_task_id
  if (!taskId) {
    return (
      <span className="font-mono text-xs text-muted-foreground" title={run.execution_id}>
        {compactId(run.execution_id)}
      </span>
    )
  }
  return (
    <Link
      to="/tasks/$taskId/executions/$executionId"
      params={{ taskId, executionId: run.execution_id }}
      className="font-mono text-xs text-primary hover:underline"
      title={run.execution_id}
    >
      {compactId(run.execution_id)}
    </Link>
  )
}

export function ProjectHooksSection({
  project,
  projectId,
  projectIsLoading,
}: {
  project?: Project
  projectId: string
  projectIsLoading: boolean
}) {
  const queryClient = useQueryClient()
  const runsQuery = useProjectHookRunsQuery(projectId)
  const agentsQuery = useAgentsQuery()
  const agentsList = agentsQuery.data?.items ?? []
  const agentOptions = useMemo<ComboSelectOption[]>(
    () =>
      agentsList.map((agent) => ({
        value: agent.id,
        label: agent.name,
        description: `${agent.executor_type} · ${agent.id}`,
      })),
    [agentsList],
  )
  const agentNames = useMemo(() => {
    const map = new Map<string, string>()
    for (const agent of agentsList) map.set(agent.id, agent.name)
    return map
  }, [agentsList])
  const rules = project?.project_hooks ?? []
  const runs = useMemo(
    () => runsQuery.data?.pages.flatMap((page) => page.items) ?? [],
    [runsQuery.data],
  )
  const runStats = useMemo(() => computeRunStats(runs), [runs])

  const [dialogMode, setDialogMode] = useState<DialogMode | null>(null)
  const [form, setForm] = useState<ProjectHookRuleFormState>(() => emptyRuleForm())
  const [formError, setFormError] = useState('')
  const [sectionError, setSectionError] = useState('')
  const [detailIndex, setDetailIndex] = useState<number | null>(null)
  const [showAllRuns, setShowAllRuns] = useState(false)

  const selectedRule = detailIndex !== null ? rules[detailIndex] : null
  const selectedRuleRuns = useMemo(
    () => (selectedRule ? runs.filter((run) => run.rule_id === selectedRule.id) : []),
    [runs, selectedRule],
  )

  const saveRules = useMutation({
    mutationFn: (nextRules: ProjectHookRuleWire[]) =>
      apiFetch<Project>(`/projects/${projectId}`, {
        method: 'PATCH',
        body: JSON.stringify({ project_hooks: nextRules }),
      }),
    onSuccess: (updatedProject) => {
      void queryClient.invalidateQueries({ queryKey: qk.project(updatedProject.id) })
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })

  const updateForm = <K extends keyof ProjectHookRuleFormState>(
    key: K,
    value: ProjectHookRuleFormState[K],
  ) => {
    setForm((current) => ({ ...current, [key]: value }))
  }

  const openAddDialog = () => {
    setDialogMode({ type: 'add' })
    setForm(emptyRuleForm())
    setFormError('')
  }

  const openEditDialog = (rule: ProjectHookRule, index: number) => {
    setDialogMode({ type: 'edit', index })
    setForm(formFromRule(rule))
    setFormError('')
  }

  const closeDialog = () => {
    if (saveRules.isPending) return
    setDialogMode(null)
    setForm(emptyRuleForm())
    setFormError('')
  }

  const submitRule = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!project || !dialogMode) return
    setFormError('')
    setSectionError('')
    const builtRule = buildRule(form)
    if (typeof builtRule === 'string') {
      setFormError(builtRule)
      return
    }

    const nextRules = rules.map(ruleToWire)
    if (dialogMode.type === 'edit') {
      if (!nextRules[dialogMode.index]) {
        setFormError('Project hook rule no longer exists')
        return
      }
      nextRules[dialogMode.index] = builtRule
    } else {
      nextRules.push(builtRule)
    }

    saveRules.mutate(nextRules, {
      onError: (error) => setFormError(serverErrorMessage(error)),
      onSuccess: () => {
        toast.success(dialogMode.type === 'edit' ? 'Project hook updated' : 'Project hook added')
        setDialogMode(null)
        setForm(emptyRuleForm())
      },
    })
  }

  const deleteRule = (index: number) => {
    if (!project || saveRules.isPending) return
    setSectionError('')
    setFormError('')
    const nextRules = rules.map(ruleToWire).filter((_, ruleIndex) => ruleIndex !== index)
    saveRules.mutate(nextRules, {
      onError: (error) => setSectionError(serverErrorMessage(error)),
      onSuccess: () => {
        toast.success('Project hook deleted')
        if (detailIndex === index) setDetailIndex(null)
      },
    })
  }

  const toggleEnabled = (index: number) => {
    if (!project || saveRules.isPending) return
    setSectionError('')
    const nextRules = rules.map(ruleToWire)
    const target = nextRules[index]
    if (!target) return
    target.enabled = !target.enabled
    saveRules.mutate(nextRules, {
      onError: (error) => setSectionError(serverErrorMessage(error)),
      onSuccess: () => toast.success(target.enabled ? 'Hook enabled' : 'Hook disabled'),
    })
  }

  return (
    <section className="mb-10 space-y-6">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-page font-semibold tracking-tight">Project hooks</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Project-wide automation rules that run when a supported project event is evaluated.
          </p>
        </div>
        {!projectIsLoading && project ? (
          <Button
            size="sm"
            className="h-8 gap-1.5"
            disabled={saveRules.isPending}
            onClick={openAddDialog}
          >
            <Plus size={14} weight="bold" aria-hidden />
            Add Rule
          </Button>
        ) : null}
      </div>

      {projectIsLoading ? (
        <Skeleton className="h-56 w-full" />
      ) : !project ? (
        <div className="rounded-md border border-dashed p-4 text-sm text-muted-foreground">
          Project hooks are unavailable until the project loads.
        </div>
      ) : (
        <>
          {sectionError ? (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {sectionError}
            </div>
          ) : null}

          {rules.length === 0 ? (
            <div className="rounded-lg border border-dashed p-6 text-center">
              <p className="text-sm font-medium">No project hooks yet</p>
              <p className="mt-1 text-sm text-muted-foreground">
                Project hooks fire when project events complete — e.g. notify when all work
                finishes, or dispatch an agent to follow up.
              </p>
              <Button size="sm" className="mt-4 gap-1.5" onClick={openAddDialog}>
                <Plus size={14} weight="bold" aria-hidden />
                Add your first hook
              </Button>
            </div>
          ) : (
            <ul className="divide-y divide-border-subtle overflow-hidden rounded-lg border border-border-subtle bg-card">
              {rules.map((rule, index) => {
                const stats = runStats.get(rule.id)
                const lastRun = stats?.lastRun ?? null
                return (
                  <li key={`${rule.id}-${index}`}>
                    <button
                      type="button"
                      onClick={() => setDetailIndex(index)}
                      className="group flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-surface-hover focus:bg-surface-hover focus:outline-none cursor-pointer"
                    >
                      <span
                        className={cn(
                          'mt-1 h-2 w-2 shrink-0 rounded-full',
                          rule.enabled
                            ? lastRun
                              ? RUN_STATUS_DOT[lastRun.status]
                              : 'bg-emerald-400'
                            : 'bg-muted-foreground/40',
                        )}
                        aria-hidden
                      />
                      <div className="min-w-0 flex-1 space-y-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="truncate font-medium">{rule.name}</span>
                          <Badge
                            variant={rule.enabled ? 'default' : 'secondary'}
                            className={cn('h-5 px-1.5 text-[10px]', rule.enabled ? '' : 'text-muted-foreground')}
                          >
                            {rule.enabled ? 'Enabled' : 'Disabled'}
                          </Badge>
                          <Badge
                            variant="outline"
                            className="h-5 px-1.5 font-mono text-[10px] text-muted-foreground"
                          >
                            {shortTrigger(rule.trigger.type)}
                          </Badge>
                          {lastRun ? (
                            <span
                              className="text-xs text-muted-foreground"
                              title={formatDate(lastRun.created_at)}
                            >
                              ran {formatRelativeTime(lastRun.created_at)}
                            </span>
                          ) : (
                            <span className="text-xs text-muted-foreground">never run</span>
                          )}
                        </div>
                        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
                          <span className="truncate">{actionSummary(rule.action, agentNames)}</span>
                          <span className="opacity-40">·</span>
                          <span>cooldown {formatCooldown(rule.cooldown_seconds)}</span>
                          <span className="opacity-40">·</span>
                          <span>concurrency {rule.max_concurrent_runs}</span>
                          {stats ? (
                            <>
                              <span className="opacity-40">·</span>
                              <span>
                                {stats.total} runs
                                {stats.completed > 0 ? (
                                  <span className="text-emerald-400"> · {stats.completed} ✓</span>
                                ) : null}
                                {stats.failed > 0 ? (
                                  <span className="text-red-400"> · {stats.failed} ✗</span>
                                ) : null}
                              </span>
                            </>
                          ) : null}
                        </div>
                      </div>
                      <div
                        className="flex items-center gap-1"
                        onClick={(event) => event.stopPropagation()}
                      >
                        <DropdownMenu>
                          <DropdownMenuTrigger
                            className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            aria-label={`Actions for ${rule.name}`}
                          >
                            <DotsThreeVertical size={16} weight="bold" aria-hidden />
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            <DropdownMenuItem onClick={() => openEditDialog(rule, index)}>
                              <PencilSimple size={14} className="mr-2" aria-hidden />
                              Edit
                            </DropdownMenuItem>
                            <DropdownMenuItem onClick={() => toggleEnabled(index)}>
                              {rule.enabled ? 'Disable' : 'Enable'}
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              className="text-destructive hover:bg-destructive/10 hover:text-destructive focus:bg-destructive/10 focus:text-destructive"
                              onClick={() => deleteRule(index)}
                            >
                              <Trash size={14} className="mr-2" aria-hidden />
                              Delete
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                        <CaretRight
                          size={14}
                          className="text-muted-foreground/60 transition-colors group-hover:text-foreground"
                          aria-hidden
                        />
                      </div>
                    </button>
                  </li>
                )
              })}
            </ul>
          )}

          <div className="space-y-3">
            <button
              type="button"
              onClick={() => setShowAllRuns((current) => !current)}
              className="inline-flex items-center gap-1.5 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground cursor-pointer"
            >
              <CaretRight
                size={12}
                weight="bold"
                aria-hidden
                className={cn('transition-transform', showAllRuns && 'rotate-90')}
              />
              {showAllRuns ? 'Hide' : 'See'} all hook runs across rules
              {runs.length > 0 ? (
                <span className="text-muted-foreground/70">({runs.length})</span>
              ) : null}
            </button>

            {showAllRuns ? (
              runsQuery.isError ? (
                <ErrorBanner
                  error={runsQuery.error}
                  fallback="Project hook runs failed to load"
                  onRetry={() => void runsQuery.refetch()}
                />
              ) : runsQuery.isLoading ? (
                <Skeleton className="h-48 w-full" />
              ) : (
                <>
                  <div className="overflow-hidden rounded-lg border border-border-subtle">
                    <div className="overflow-x-auto">
                      <table className="w-full min-w-[980px] text-left text-sm">
                        <thead className="border-b bg-muted/40 text-xs uppercase tracking-wide text-muted-foreground">
                          <tr>
                            <th className="px-3 py-2 font-medium">Created</th>
                            <th className="px-3 py-2 font-medium">Rule</th>
                            <th className="px-3 py-2 font-medium">Trigger</th>
                            <th className="px-3 py-2 font-medium">Dedupe key</th>
                            <th className="px-3 py-2 font-medium">Status</th>
                            <th className="px-3 py-2 font-medium">Source task</th>
                            <th className="px-3 py-2 font-medium">Automation task</th>
                            <th className="px-3 py-2 font-medium">{productTerm('run')}</th>
                            <th className="px-3 py-2 font-medium">Reason</th>
                          </tr>
                        </thead>
                        <tbody className="divide-y divide-border-subtle">
                          {runs.length === 0 ? (
                            <tr>
                              <td
                                colSpan={9}
                                className="px-3 py-6 text-center text-sm text-muted-foreground"
                              >
                                No hook runs recorded
                              </td>
                            </tr>
                          ) : (
                            runs.map((run) => (
                              <tr key={run.id} className="bg-card">
                                <td
                                  className="whitespace-nowrap px-3 py-2 text-muted-foreground"
                                  title={formatDate(run.created_at)}
                                >
                                  {formatRelativeTime(run.created_at)}
                                </td>
                                <td className="px-3 py-2 font-mono text-xs">{run.rule_id}</td>
                                <td className="px-3 py-2 font-mono text-xs text-muted-foreground">
                                  {run.trigger_type}
                                </td>
                                <td
                                  className="max-w-[180px] truncate px-3 py-2 font-mono text-xs text-muted-foreground"
                                  title={run.dedupe_key}
                                >
                                  {run.dedupe_key}
                                </td>
                                <td className="px-3 py-2">
                                  <Badge
                                    variant="outline"
                                    className={cn('capitalize', RUN_STATUS_CLASSES[run.status])}
                                  >
                                    {run.status}
                                  </Badge>
                                </td>
                                <td className="px-3 py-2">
                                  <TaskLink taskId={run.source_task_id} />
                                </td>
                                <td className="px-3 py-2">
                                  <TaskLink taskId={run.automation_task_id} />
                                </td>
                                <td className="px-3 py-2">
                                  <ExecutionLink run={run} />
                                </td>
                                <td
                                  className="max-w-[220px] truncate px-3 py-2 text-muted-foreground"
                                  title={run.reason ?? undefined}
                                >
                                  {run.reason ?? '-'}
                                </td>
                              </tr>
                            ))
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>

                  {runsQuery.hasNextPage ? (
                    <div className="flex justify-center">
                      <Button
                        variant="outline"
                        disabled={runsQuery.isFetchingNextPage}
                        onClick={() => void runsQuery.fetchNextPage()}
                      >
                        {runsQuery.isFetchingNextPage ? 'Loading...' : 'Load More'}
                      </Button>
                    </div>
                  ) : null}
                </>
              )
            ) : null}
          </div>
        </>
      )}

      <Sheet
        open={selectedRule !== null}
        onOpenChange={(open) => {
          if (!open) setDetailIndex(null)
        }}
      >
        <SheetContent>
          {selectedRule ? (
            <>
              <SheetHeader>
                <div className="flex items-center gap-2">
                  <SheetTitle className="truncate">{selectedRule.name}</SheetTitle>
                  <Badge
                    variant={selectedRule.enabled ? 'default' : 'secondary'}
                    className={cn(
                      'h-5 px-1.5 text-[10px]',
                      selectedRule.enabled ? '' : 'text-muted-foreground',
                    )}
                  >
                    {selectedRule.enabled ? 'Enabled' : 'Disabled'}
                  </Badge>
                </div>
                <p className="font-mono text-xs text-muted-foreground" title={selectedRule.id}>
                  {selectedRule.id}
                </p>
              </SheetHeader>
              <SheetBody className="space-y-6">
                <div className="space-y-3">
                  <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                    Configuration
                  </p>
                  <dl className="grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 text-sm">
                    <dt className="text-muted-foreground">Trigger</dt>
                    <dd className="font-mono text-xs">{selectedRule.trigger.type}</dd>

                    <dt className="text-muted-foreground">Action</dt>
                    <dd className="font-mono text-xs">{selectedRule.action.type}</dd>

                    {selectedRule.action.type === 'dispatch_agent' ? (
                      <>
                        <dt className="text-muted-foreground">Agent</dt>
                        <dd className="text-xs">
                          {(() => {
                            const agentId = selectedRule.action.agent_id
                            const match = agentsList.find((a) => a.id === agentId)
                            if (match) {
                              return (
                                <span>
                                  <span>{match.name}</span>
                                  <span className="ml-1 font-mono text-muted-foreground">({agentId})</span>
                                </span>
                              )
                            }
                            return <span className="font-mono">{agentId}</span>
                          })()}
                        </dd>
                        {selectedRule.action.prompt ? (
                          <>
                            <dt className="text-muted-foreground">Prompt</dt>
                            <dd className="whitespace-pre-wrap rounded-md border bg-muted/30 p-2 text-xs">
                              {selectedRule.action.prompt}
                            </dd>
                          </>
                        ) : null}
                      </>
                    ) : null}

                    {selectedRule.action.type === 'create_task' ? (
                      <>
                        <dt className="text-muted-foreground">Title</dt>
                        <dd className="text-xs">{selectedRule.action.title}</dd>
                        {selectedRule.action.description ? (
                          <>
                            <dt className="text-muted-foreground">Description</dt>
                            <dd className="whitespace-pre-wrap rounded-md border bg-muted/30 p-2 text-xs">
                              {selectedRule.action.description}
                            </dd>
                          </>
                        ) : null}
                        {selectedRule.action.priority != null ? (
                          <>
                            <dt className="text-muted-foreground">Priority</dt>
                            <dd className="text-xs">{String(selectedRule.action.priority)}</dd>
                          </>
                        ) : null}
                      </>
                    ) : null}

                    {selectedRule.action.type === 'add_comment' ? (
                      <>
                        <dt className="text-muted-foreground">Target</dt>
                        <dd className="font-mono text-xs">
                          {selectedRule.action.target_task_id ?? 'source task'}
                        </dd>
                        <dt className="text-muted-foreground">Content</dt>
                        <dd className="whitespace-pre-wrap rounded-md border bg-muted/30 p-2 text-xs">
                          {selectedRule.action.content}
                        </dd>
                      </>
                    ) : null}

                    {selectedRule.action.type === 'notify' ? (
                      <>
                        <dt className="text-muted-foreground">Title</dt>
                        <dd className="text-xs">{selectedRule.action.title}</dd>
                        <dt className="text-muted-foreground">Message</dt>
                        <dd className="whitespace-pre-wrap rounded-md border bg-muted/30 p-2 text-xs">
                          {selectedRule.action.message}
                        </dd>
                        {selectedRule.action.severity ? (
                          <>
                            <dt className="text-muted-foreground">Severity</dt>
                            <dd className="text-xs capitalize">{selectedRule.action.severity}</dd>
                          </>
                        ) : null}
                      </>
                    ) : null}

                    <dt className="text-muted-foreground">Cooldown</dt>
                    <dd className="text-xs">{formatCooldown(selectedRule.cooldown_seconds)}</dd>

                    <dt className="text-muted-foreground">Concurrency</dt>
                    <dd className="text-xs">{selectedRule.max_concurrent_runs}</dd>
                  </dl>
                </div>

                <div className="space-y-3">
                  <div className="flex items-center justify-between">
                    <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                      Recent runs
                    </p>
                    <span className="text-xs text-muted-foreground">
                      {selectedRuleRuns.length} shown
                    </span>
                  </div>
                  {runsQuery.isLoading ? (
                    <Skeleton className="h-24 w-full" />
                  ) : selectedRuleRuns.length === 0 ? (
                    <div className="rounded-md border border-dashed p-4 text-center text-sm text-muted-foreground">
                      This rule hasn&apos;t run yet.
                    </div>
                  ) : (
                    <ul className="divide-y divide-border-subtle overflow-hidden rounded-lg border border-border-subtle">
                      {selectedRuleRuns.map((run) => (
                        <li key={run.id} className="space-y-1 px-3 py-2 text-xs">
                          <div className="flex flex-wrap items-center gap-2">
                            <span
                              className={cn('h-1.5 w-1.5 rounded-full', RUN_STATUS_DOT[run.status])}
                              aria-hidden
                            />
                            <Badge
                              variant="outline"
                              className={cn(
                                'h-5 px-1.5 text-[10px] capitalize',
                                RUN_STATUS_CLASSES[run.status],
                              )}
                            >
                              {run.status}
                            </Badge>
                            <span
                              className="text-muted-foreground"
                              title={formatDate(run.created_at)}
                            >
                              {formatRelativeTime(run.created_at)}
                            </span>
                          </div>
                          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground">
                            {run.source_task_id ? (
                              <span>
                                src{' '}
                                <TaskLink taskId={run.source_task_id} />
                              </span>
                            ) : null}
                            {run.automation_task_id ? (
                              <span>
                                task{' '}
                                <TaskLink taskId={run.automation_task_id} />
                              </span>
                            ) : null}
                            {run.execution_id ? (
                              <span>
                                {productTerm('run').toLowerCase()} <ExecutionLink run={run} />
                              </span>
                            ) : null}
                          </div>
                          {run.reason ? (
                            <p className="text-muted-foreground" title={run.reason}>
                              {run.reason}
                            </p>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  )}
                  {runsQuery.hasNextPage ? (
                    <div className="flex justify-center">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={runsQuery.isFetchingNextPage}
                        onClick={() => void runsQuery.fetchNextPage()}
                      >
                        {runsQuery.isFetchingNextPage ? 'Loading...' : 'Load more runs'}
                      </Button>
                    </div>
                  ) : null}
                </div>
              </SheetBody>
              <SheetFooter>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setDetailIndex(null)}
                >
                  Close
                </Button>
                <Button
                  size="sm"
                  onClick={() => {
                    if (detailIndex !== null) openEditDialog(selectedRule, detailIndex)
                  }}
                >
                  <PencilSimple size={14} className="mr-1.5" aria-hidden />
                  Edit
                </Button>
              </SheetFooter>
            </>
          ) : null}
        </SheetContent>
      </Sheet>

      <Dialog
        open={dialogMode !== null}
        onOpenChange={(open) => {
          if (!open) closeDialog()
        }}
      >
        <DialogContent className="sm:max-w-2xl">
          <form className="space-y-4" onSubmit={submitRule}>
            <DialogHeader>
              <DialogTitle>
                {dialogMode?.type === 'edit' ? 'Edit Project Hook' : 'Add Project Hook'}
              </DialogTitle>
              <DialogDescription>
                Configure the project-wide trigger rule and the action Forge should run.
              </DialogDescription>
            </DialogHeader>

            {formError ? (
              <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {formError}
              </div>
            ) : null}

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="project-hook-id">Id</Label>
                <Input
                  id="project-hook-id"
                  required
                  value={form.id}
                  onChange={(event) => updateForm('id', event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="project-hook-name">Name</Label>
                <Input
                  id="project-hook-name"
                  required
                  value={form.name}
                  onChange={(event) => updateForm('name', event.target.value)}
                />
              </div>
            </div>

            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                checked={form.enabled}
                onChange={(event) => updateForm('enabled', event.target.checked)}
              />
              Enabled
            </label>

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="project-hook-trigger">Trigger type</Label>
                <Select
                  id="project-hook-trigger"
                  value={form.triggerType}
                  options={TRIGGER_OPTIONS}
                  onChange={() => updateForm('triggerType', 'project.all_work_completed')}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="project-hook-action">Action type</Label>
                <Select
                  id="project-hook-action"
                  value={form.actionType}
                  options={ACTION_OPTIONS}
                  onChange={(value) => {
                    if (isActionType(value)) updateForm('actionType', value)
                  }}
                />
              </div>
            </div>

            {form.actionType === 'dispatch_agent' ? (
              <div className="space-y-4 rounded-md border p-3">
                <div className="space-y-2">
                  <Label htmlFor="project-hook-agent-id">Agent</Label>
                  <ComboSelect
                    id="project-hook-agent-id"
                    value={form.dispatchAgentId || null}
                    options={agentOptions}
                    onChange={(value) => updateForm('dispatchAgentId', value ?? '')}
                    placeholder={
                      agentsQuery.isLoading
                        ? 'Loading agents...'
                        : agentOptions.length === 0
                          ? 'No agents available'
                          : 'Select an agent'
                    }
                    isLoading={agentsQuery.isLoading}
                    allowCustom
                  />
                  <p className="text-xs text-muted-foreground">
                    Pick any agent you have access to, or type a custom agent ID.
                  </p>
                </div>
                <div className="space-y-2">
                  <Label htmlFor="project-hook-dispatch-prompt">Prompt</Label>
                  <Textarea
                    id="project-hook-dispatch-prompt"
                    value={form.dispatchPrompt}
                    onChange={(event) => updateForm('dispatchPrompt', event.target.value)}
                  />
                </div>
              </div>
            ) : null}

            {form.actionType === 'create_task' ? (
              <div className="space-y-4 rounded-md border p-3">
                <div className="space-y-2">
                  <Label htmlFor="project-hook-task-title">Title</Label>
                  <Input
                    id="project-hook-task-title"
                    required
                    value={form.createTaskTitle}
                    onChange={(event) => updateForm('createTaskTitle', event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="project-hook-task-description">Description</Label>
                  <Textarea
                    id="project-hook-task-description"
                    value={form.createTaskDescription}
                    onChange={(event) => updateForm('createTaskDescription', event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="project-hook-task-priority">Priority</Label>
                  <Select
                    id="project-hook-task-priority"
                    value={form.createTaskPriority}
                    options={PRIORITY_OPTIONS}
                    onChange={(value) => {
                      if (isPriorityOption(value)) updateForm('createTaskPriority', value)
                    }}
                  />
                </div>
              </div>
            ) : null}

            {form.actionType === 'add_comment' ? (
              <div className="space-y-4 rounded-md border p-3">
                <div className="space-y-2">
                  <Label htmlFor="project-hook-target-task">Target task ID</Label>
                  <Input
                    id="project-hook-target-task"
                    value={form.addCommentTargetTaskId}
                    onChange={(event) => updateForm('addCommentTargetTaskId', event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="project-hook-comment-content">Content</Label>
                  <Textarea
                    id="project-hook-comment-content"
                    required
                    value={form.addCommentContent}
                    onChange={(event) => updateForm('addCommentContent', event.target.value)}
                  />
                </div>
              </div>
            ) : null}

            {form.actionType === 'notify' ? (
              <div className="space-y-4 rounded-md border p-3">
                <div className="space-y-2">
                  <Label htmlFor="project-hook-notify-title">Title</Label>
                  <Input
                    id="project-hook-notify-title"
                    required
                    value={form.notifyTitle}
                    onChange={(event) => updateForm('notifyTitle', event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="project-hook-notify-message">Message</Label>
                  <Textarea
                    id="project-hook-notify-message"
                    required
                    value={form.notifyMessage}
                    onChange={(event) => updateForm('notifyMessage', event.target.value)}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="project-hook-notify-severity">Severity</Label>
                  <Select
                    id="project-hook-notify-severity"
                    value={form.notifySeverity}
                    options={SEVERITY_OPTIONS}
                    onChange={(value) => {
                      if (isSeverityOption(value)) updateForm('notifySeverity', value)
                    }}
                  />
                </div>
              </div>
            ) : null}

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="project-hook-cooldown">Cooldown seconds</Label>
                <Input
                  id="project-hook-cooldown"
                  type="number"
                  min={0}
                  value={form.cooldownSeconds}
                  onChange={(event) => updateForm('cooldownSeconds', event.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="project-hook-max-runs">Max concurrent runs</Label>
                <Input
                  id="project-hook-max-runs"
                  type="number"
                  min={1}
                  max={10}
                  value={form.maxConcurrentRuns}
                  onChange={(event) => updateForm('maxConcurrentRuns', event.target.value)}
                />
              </div>
            </div>

            <DialogFooter>
              <Button type="button" variant="outline" onClick={closeDialog}>
                Cancel
              </Button>
              <Button type="submit" disabled={saveRules.isPending || !project}>
                {saveRules.isPending ? 'Saving...' : 'Save'}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </section>
  )
}
