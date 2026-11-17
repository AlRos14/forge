import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import {
  ArrowClockwise,
  ChatCircleDots,
  CircleNotch,
  PaperPlaneRight,
  PushPin,
  Robot,
  Sparkle,
  X,
} from '@phosphor-icons/react'
import { useTranslation } from 'react-i18next'
import { ApiError, apiFetch } from '@/api/client'
import { qk } from '@/api/query-keys'
import { useAgentsInfiniteQuery } from '@/api/hooks'
import { AgentSelector } from '@/components/execution-config/AgentSelector'
import { Button } from '@/components/ui/button'
import { Dialog, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { MarkdownView } from '@/components/ui/markdown-editor'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  ExecutionLogEntry,
  Project,
  ProjectCopilotActivity,
  ProjectMemberResponse,
  Task,
} from '@/types/generated'
import { useAuthStore } from '@/stores/auth'
import {
  useProjectCopilotMessagesQuery,
  useProjectCopilotActivityQuery,
  useProjectCopilotQuery,
  useProjectCopilotRunLogsQuery,
  useProjectCopilotRunQuery,
  useCancelProjectCopilotRunMutation,
  useSendProjectCopilotMessageMutation,
  useUpdateProjectCopilotMutation,
} from '@/features/project-copilot/hooks'
import type {
  ProjectCopilotRunDetailResponse,
  ProjectCopilotRunStatus,
  SendProjectCopilotMessageRequest,
} from '@/features/project-copilot/types'
import {
  isProjectCopilotRunPending,
  isProjectCopilotRunTerminal,
} from '@/features/project-copilot/types'
import { cn } from '@/lib/cn'

const RUNNING_STATUSES = new Set<ProjectCopilotRunStatus>(['queued', 'running'])
const ACTIVITY_TOOL_LABEL_KEYS: Record<string, string> = {
  forge_get_project: 'projectCopilot.activity.tools.readProject',
  forge_get_task: 'projectCopilot.activity.tools.readTask',
  forge_list_task_roles: 'projectCopilot.activity.tools.readTaskRoles',
  forge_list_work_units: 'projectCopilot.activity.tools.readWorkUnits',
  forge_get_work_unit: 'projectCopilot.activity.tools.readWorkUnit',
  forge_create_work_unit: 'projectCopilot.activity.tools.createWorkUnit',
  forge_update_work_unit: 'projectCopilot.activity.tools.updateWorkUnit',
  forge_allocate_work_unit: 'projectCopilot.activity.tools.allocateWorkUnit',
  forge_list_work_unit_dependencies: 'projectCopilot.activity.tools.readDependencies',
  forge_add_work_unit_dependency: 'projectCopilot.activity.tools.addDependency',
  forge_remove_work_unit_dependency: 'projectCopilot.activity.tools.removeDependency',
}

type TaskContext = { id: string; title: string }

function idempotencyKey() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`
}

function useCompactViewport() {
  const [isCompactViewport, setIsCompactViewport] = useState(
    () => typeof window !== 'undefined' && window.innerWidth < 640,
  )

  useEffect(() => {
    const updateViewport = () => setIsCompactViewport(window.innerWidth < 640)
    window.addEventListener('resize', updateViewport)
    return () => window.removeEventListener('resize', updateViewport)
  }, [])

  return isCompactViewport
}

function runStatusLabel(t: (key: string) => string, status: ProjectCopilotRunStatus) {
  return t(`projectCopilot.runStatus.${status}`)
}

function traceEntryLabel(entry: ExecutionLogEntry) {
  const payload = (entry.payload ?? {}) as Record<string, unknown>
  const eventType = typeof payload.type === 'string' ? payload.type : null
  return eventType ?? entry.kind
}

function traceEntryText(entry: ExecutionLogEntry) {
  const payload = (entry.payload ?? {}) as Record<string, unknown>
  const part =
    payload.part && typeof payload.part === 'object'
      ? (payload.part as Record<string, unknown>)
      : undefined
  const text = part?.text ?? payload.text ?? payload.line ?? payload.message
  if (typeof text === 'string' && text.trim()) return text
  const tool = part?.tool ?? payload.tool ?? payload.name
  if (typeof tool === 'string' && tool.trim()) {
    const input = part?.state ?? payload.input ?? payload.arguments
    return input ? `${tool} ${JSON.stringify(input)}` : tool
  }
  try {
    return JSON.stringify(payload)
  } catch {
    return ''
  }
}

function CopilotTrigger({
  open,
  disabled = false,
  label,
  onClick,
}: {
  open: boolean
  disabled?: boolean
  label?: string
  onClick: () => void
}) {
  const { t } = useTranslation()
  return (
    <Button
      type="button"
      size="default"
      className="fixed bottom-4 right-4 z-40 h-11 rounded-full px-4 shadow-lg focus-visible:ring-offset-4"
      aria-label={label ?? t('projectCopilot.open')}
      aria-haspopup="dialog"
      aria-expanded={open}
      disabled={disabled}
      onClick={onClick}
    >
      <Sparkle size={17} weight="fill" aria-hidden="true" />
      <span>{t('projectCopilot.button')}</span>
    </Button>
  )
}

/** Keeps the Project Copilot available from all Project and Task routes. */
export function ProjectCopilotDock({ projectId, taskId }: { projectId?: string; taskId?: string }) {
  const { t } = useTranslation()
  const taskQuery = useQuery({
    queryKey: qk.task(taskId ?? ''),
    queryFn: () => apiFetch<Task>(`/tasks/${encodeURIComponent(taskId!)}`),
    enabled: Boolean(taskId && !projectId),
  })
  const resolvedProjectId = projectId ?? taskQuery.data?.project_id
  const [lastResolvedProjectId, setLastResolvedProjectId] = useState(projectId)

  useEffect(() => {
    if (resolvedProjectId) setLastResolvedProjectId(resolvedProjectId)
  }, [resolvedProjectId])

  const sessionProjectId = resolvedProjectId ?? lastResolvedProjectId

  if (!sessionProjectId) {
    if (!taskId) return null
    return (
      <CopilotTrigger
        open={false}
        disabled={taskQuery.isLoading}
        label={t(
          taskQuery.isError ? 'projectCopilot.retryTaskContext' : 'projectCopilot.loadingTask',
        )}
        onClick={() => void taskQuery.refetch()}
      />
    )
  }

  const task = taskId && taskQuery.data?.id === taskId ? taskQuery.data : undefined
  return (
    <>
      <ProjectCopilotSession
        key={sessionProjectId}
        projectId={sessionProjectId}
        taskId={taskId}
        active={Boolean(resolvedProjectId)}
        task={task}
        taskContextReady={!taskId || Boolean(task && !taskQuery.error)}
        taskLoading={Boolean(taskId && taskQuery.isLoading)}
        taskError={taskId ? taskQuery.error : null}
        retryTask={() => void taskQuery.refetch()}
      />
      {!resolvedProjectId ? (
        <CopilotTrigger
          open={false}
          disabled={taskQuery.isLoading}
          label={t(
            taskQuery.isError ? 'projectCopilot.retryTaskContext' : 'projectCopilot.loadingTask',
          )}
          onClick={() => void taskQuery.refetch()}
        />
      ) : null}
    </>
  )
}

function ProjectCopilotSession({
  projectId,
  taskId,
  active,
  task,
  taskContextReady,
  taskLoading,
  taskError,
  retryTask,
}: {
  projectId: string
  taskId?: string
  active: boolean
  task?: Task
  taskContextReady: boolean
  taskLoading: boolean
  taskError: unknown
  retryTask: () => void
}) {
  const isCompactViewport = useCompactViewport()
  const { t } = useTranslation()
  const user = useAuthStore((state) => state.user)
  const queryClient = useQueryClient()
  const [open, setOpen] = useState(false)
  const [pinned, setPinned] = useState(false)
  const [draft, setDraft] = useState('')
  const [draftTaskContext, setDraftTaskContext] = useState<TaskContext | null>(
    task ? { id: task.id, title: task.title } : null,
  )
  const [agentSelection, setAgentSelection] = useState<string | null>(null)
  const [editingAgent, setEditingAgent] = useState(false)
  const [trackedRunId, setTrackedRunId] = useState<string | null>(null)
  const [terminalRun, setTerminalRun] = useState<ProjectCopilotRunDetailResponse | null>(null)
  const [traceOpen, setTraceOpen] = useState(false)
  const [retryPayload, setRetryPayload] = useState<SendProjectCopilotMessageRequest | null>(null)

  useEffect(() => {
    setTrackedRunId(null)
    setTerminalRun(null)
  }, [projectId])
  const [sendError, setSendError] = useState<unknown>(null)
  const [configError, setConfigError] = useState<unknown>(null)
  const messageListRef = useRef<HTMLDivElement>(null)
  const historyScrollHeightRef = useRef<number | null>(null)
  const activeTaskIdRef = useRef<string | null>(taskId ?? null)
  const processedTerminalRunRef = useRef<string | null>(null)

  const configQuery = useProjectCopilotQuery(projectId, open && active)
  const messagesQuery = useProjectCopilotMessagesQuery(projectId, open && active)
  const activityQuery = useProjectCopilotActivityQuery(projectId, open && active)
  const updateCopilot = useUpdateProjectCopilotMutation(projectId)
  const sendMessage = useSendProjectCopilotMessageMutation(projectId)
  const cancelRun = useCancelProjectCopilotRunMutation(projectId)
  const projectQuery = useQuery({
    queryKey: qk.project(projectId),
    queryFn: () => apiFetch<Project>(`/projects/${encodeURIComponent(projectId)}`),
    enabled: active,
  })
  const membersQuery = useQuery({
    queryKey: qk.projectMembers(projectId),
    queryFn: () =>
      apiFetch<ProjectMemberResponse[]>(`/projects/${encodeURIComponent(projectId)}/members`),
    enabled: active,
  })
  const project = projectQuery.data
  const currentMember = membersQuery.data?.find((member) => member.user_id === user?.id)
  const isCanonicalOwner = currentMember?.role === 'owner' && project?.owner_id === user?.id
  const canManage = Boolean(currentMember?.role === 'admin' || isCanonicalOwner)
  const canWrite = Boolean(
    currentMember?.role === 'member' || currentMember?.role === 'admin' || isCanonicalOwner,
  )
  const config = configQuery.data

  useEffect(() => {
    if (config === undefined) return
    setAgentSelection(config.agent_id)
    setEditingAgent(config.agent_id === null)
  }, [config?.project_id, config?.agent_id, config?.version])

  useEffect(() => {
    const nextTaskId = taskId ?? null
    if (activeTaskIdRef.current !== nextTaskId) {
      if (!pinned) setOpen(false)
      // A Project route (including the plan page) always drafts with Project-only
      // authority. Keep draft text if the user already typed, but detach the
      // prior Task so opening Copilot from the Project cannot target stale scope.
      if (nextTaskId === null) setDraftTaskContext(null)
      else if (!draft.trim()) setDraftTaskContext(null)
      activeTaskIdRef.current = nextTaskId
    }
    // Route Task IDs change before their query resolves. Keep a nonempty draft
    // bound to its original Task across that loading gap; when the draft is
    // empty, bind it to the newly resolved Task as soon as it is available.
    if (nextTaskId && task?.id === nextTaskId && !draft.trim()) {
      setDraftTaskContext({ id: task.id, title: task.title })
    }
  }, [draft, pinned, task?.id, task?.title, taskId])

  const activeRunSummary = config?.active_run
  const activeRunId =
    trackedRunId ??
    (activeRunSummary && RUNNING_STATUSES.has(activeRunSummary.status) ? activeRunSummary.id : null)
  const runQuery = useProjectCopilotRunQuery(projectId, activeRunId, open && active)
  const run =
    runQuery.data?.run ?? (activeRunSummary?.id === activeRunId ? activeRunSummary : undefined)
  const runIsPending = isProjectCopilotRunPending(run?.status)
  const hasPendingWork =
    runIsPending ||
    sendMessage.isPending ||
    Boolean(retryPayload) ||
    updateCopilot.isPending ||
    Boolean(configError)
  const messages = useMemo(
    () => [...(messagesQuery.data?.pages ?? [])].reverse().flatMap((page) => page.items),
    [messagesQuery.data],
  )

  useEffect(() => {
    if (!open) return
    const list = messageListRef.current
    if (!list) return
    const previousHeight = historyScrollHeightRef.current
    if (previousHeight !== null) {
      if (messagesQuery.isFetchingNextPage) return
      list.scrollTop += list.scrollHeight - previousHeight
      historyScrollHeightRef.current = null
      return
    }
    list.scrollTop = list.scrollHeight
  }, [messages.length, messagesQuery.isFetchingNextPage, open, run?.status])

  useEffect(() => {
    const detail = runQuery.data
    if (!detail || !isProjectCopilotRunTerminal(detail.run.status)) return
    if (processedTerminalRunRef.current === detail.run.id) return
    processedTerminalRunRef.current = detail.run.id
    setTerminalRun(detail)
    setTrackedRunId(null)
    void messagesQuery.refetch()
    void activityQuery.refetch()
    void configQuery.refetch()
    void queryClient.invalidateQueries({ queryKey: qk.projectTasks(projectId) })
    const contextTaskId = detail.run.context_task_id
    if (contextTaskId) {
      void queryClient.invalidateQueries({ queryKey: qk.task(contextTaskId) })
      void queryClient.invalidateQueries({ queryKey: qk.workUnits(contextTaskId) })
    }
  }, [activityQuery, configQuery, messagesQuery, projectId, queryClient, runQuery.data])

  const send = (payload: SendProjectCopilotMessageRequest) => {
    setRetryPayload(payload)
    setSendError(null)
    sendMessage.mutate(payload, {
      onSuccess: (response) => {
        setTerminalRun(null)
        setTrackedRunId(response.run.id)
        setRetryPayload(null)
        setSendError(null)
        if (draft === payload.content) {
          setDraft('')
          setDraftTaskContext(task ? { id: task.id, title: task.title } : null)
        }
        void messagesQuery.refetch()
      },
      onError: (error) => {
        setSendError(error)
        if (
          error instanceof ApiError &&
          error.status >= 400 &&
          error.status < 500 &&
          error.status !== 408 &&
          error.status !== 429
        ) {
          // A rejected request can be edited and sent with a new key. Unknown
          // outcomes retain the original payload/key for a safe retry.
          setRetryPayload(null)
        }
      },
    })
  }

  const cancelActiveRun = () => {
    if (!activeRunId || !canWrite || cancelRun.isPending || !runIsPending) return
    cancelRun.mutate(activeRunId)
  }

  const loadOlderMessages = () => {
    const list = messageListRef.current
    if (list) historyScrollHeightRef.current = list.scrollHeight
    void messagesQuery.fetchNextPage()
  }

  const submitDraft = () => {
    const content = draft.trim()
    if (
      !content ||
      !config?.agent_id ||
      !canWrite ||
      !taskContextReady ||
      hasPendingWork ||
      sendMessage.isPending
    )
      return
    send({
      content,
      idempotency_key: idempotencyKey(),
      task_id: draftTaskContext?.id ?? null,
    })
  }

  const organizeTask = () => {
    if (!task || !taskContextReady || !canWrite || !config?.agent_id || hasPendingWork) return
    send({
      content: t('projectCopilot.organizeTaskPrompt'),
      idempotency_key: idempotencyKey(),
      task_id: task.id,
    })
  }

  const saveAgent = () => {
    if (!config || !canManage || updateCopilot.isPending) return
    updateCopilot.mutate(
      { expected_version: config.version, agent_id: agentSelection },
      {
        onSuccess: () => {
          setConfigError(null)
          setEditingAgent(false)
        },
        onError: (error) => setConfigError(error),
      },
    )
  }

  const currentTaskContextId = task?.id ?? null
  const draftContextMatchesCurrent = draftTaskContext?.id === currentTaskContextId
  const showDraftContextNotice = Boolean(
    draft.trim() && (draftTaskContext || currentTaskContextId) && !draftContextMatchesCurrent,
  )
  const terminalRunDetail = runQuery.data ?? terminalRun
  const runFailureMessage =
    terminalRunDetail?.run.failure_message ?? terminalRunDetail?.run.failure_code ?? ''
  const traceRunId = activeRunId ?? terminalRun?.run.id ?? null
  const traceQuery = useProjectCopilotRunLogsQuery(
    projectId,
    traceRunId,
    open && active && traceOpen,
    Boolean(runIsPending),
  )
  const traceItems = traceQuery.data?.items ?? []
  const activityItems = activityQuery.data?.pages.flatMap((page) => page.items) ?? []

  const activityLabel = (item: ProjectCopilotActivity) => {
    if (item.event_type === 'work_unit.created') return t('projectCopilot.activity.workUnitCreated')
    if (item.event_type === 'work_unit.allocated')
      return t('projectCopilot.activity.workUnitAllocated')
    if (item.event_type === 'work_unit.updated') return t('projectCopilot.activity.workUnitUpdated')
    if (item.event_type === 'work_unit.dependency_added')
      return t('projectCopilot.activity.dependencyAdded')
    if (item.event_type === 'work_unit.dependency_removed')
      return t('projectCopilot.activity.dependencyRemoved')
    const toolKey = item.tool ? ACTIVITY_TOOL_LABEL_KEYS[item.tool] : undefined
    const toolLabel = toolKey ? t(toolKey) : t('projectCopilot.activity.managementAction')
    if (item.event_type === 'project_copilot.tool_started') {
      return t('projectCopilot.activity.toolStarted', { tool: toolLabel })
    }
    if (item.event_type === 'project_copilot.tool_completed') {
      return t('projectCopilot.activity.toolCompleted', { tool: toolLabel })
    }
    if (item.event_type === 'project_copilot.tool_failed') {
      return t('projectCopilot.activity.toolFailed', { tool: toolLabel })
    }
    return t('projectCopilot.activity.projectUpdated')
  }

  const messageActorLabel = (message: (typeof messages)[number]) => {
    if (message.actor_type === 'agent' || message.role === 'assistant') {
      return t('projectCopilot.copilotActor')
    }
    if (message.actor_type === 'human' && message.actor_id === user?.id) {
      return t('projectCopilot.you')
    }
    const member = membersQuery.data?.find((candidate) => candidate.user_id === message.actor_id)
    if (member) return member.display_name || member.email
    return t('projectCopilot.otherHuman', { id: message.actor_id.slice(0, 8) })
  }

  const activityRequesterName = (requestedByUserId: string) => {
    if (requestedByUserId === user?.id) return t('projectCopilot.you')
    const member = membersQuery.data?.find((candidate) => candidate.user_id === requestedByUserId)
    return (
      member?.display_name ||
      member?.email ||
      t('projectCopilot.activity.projectMember', { id: requestedByUserId.slice(0, 8) })
    )
  }

  return (
    <>
      {active ? <CopilotTrigger open={open} onClick={() => setOpen(true)} /> : null}
      <Dialog
        open={active && open}
        onOpenChange={setOpen}
        modal={isCompactViewport}
        ariaLabel={t('projectCopilot.dialogLabel')}
        className="fixed inset-y-0 right-0 flex h-[100dvh] max-h-none w-full max-w-[min(100vw,36rem)] translate-x-0 flex-col overflow-hidden rounded-none border-l p-0 shadow-xl animate-none sm:w-[min(100vw,36rem)]"
      >
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex shrink-0 items-start justify-between gap-3 border-b px-4 py-4 sm:px-5">
            <DialogHeader className="min-w-0 flex-1 space-y-1 text-left">
              <DialogTitle className="flex items-center gap-2 text-base">
                <Robot size={18} className="text-primary" aria-hidden="true" />
                {t('projectCopilot.title')}
              </DialogTitle>
              <DialogDescription className="text-xs">
                {project?.name
                  ? t('projectCopilot.projectDescription', { project: project.name })
                  : t('projectCopilot.description')}
              </DialogDescription>
            </DialogHeader>
            <div className="flex shrink-0 items-center gap-1">
              <Button
                size="icon"
                variant={pinned ? 'secondary' : 'ghost'}
                aria-label={pinned ? t('projectCopilot.unpin') : t('projectCopilot.pin')}
                aria-pressed={pinned}
                title={pinned ? t('projectCopilot.unpin') : t('projectCopilot.pin')}
                onClick={() => setPinned((value) => !value)}
              >
                <PushPin size={16} weight={pinned ? 'fill' : 'regular'} />
              </Button>
              <Button
                size="icon"
                variant="ghost"
                aria-label={t('projectCopilot.close')}
                onClick={() => setOpen(false)}
              >
                <X size={17} />
              </Button>
            </div>
          </div>

          <div className="shrink-0 border-b px-4 py-3 sm:px-5">
            <div className="flex items-center justify-between gap-3">
              <div className="min-w-0 text-sm">
                <span className="text-muted-foreground">{t('projectCopilot.agentLabel')}: </span>
                <span className="font-medium" title={config?.agent_name ?? undefined}>
                  {config?.agent_name ?? t('projectCopilot.noAgent')}
                </span>
              </div>
              {config?.agent_id && canManage && !editingAgent ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={hasPendingWork || updateCopilot.isPending}
                  onClick={() => {
                    setAgentSelection(config.agent_id)
                    setConfigError(null)
                    setEditingAgent(true)
                  }}
                >
                  {t('projectCopilot.changeAgent')}
                </Button>
              ) : null}
            </div>

            {editingAgent && config ? (
              <AgentAssignmentEditor
                projectId={projectId}
                ownerId={project?.owner_id ?? null}
                supportedExecutorTypes={config.supported_executor_types}
                supportedDaemonId={config.supported_daemon_id}
                value={agentSelection}
                currentAgentId={config.agent_id}
                canManage={canManage}
                isSaving={updateCopilot.isPending}
                onChange={setAgentSelection}
                onSave={saveAgent}
                onCancel={() => {
                  setAgentSelection(config.agent_id)
                  setConfigError(null)
                  setEditingAgent(false)
                }}
              />
            ) : null}

            {configError ? (
              <div
                role="alert"
                className="mt-3 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                <span>{getApiErrorMessage(configError, t('projectCopilot.saveAgentFailed'))}</span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    void configQuery.refetch()
                    setConfigError(null)
                    setEditingAgent(false)
                  }}
                >
                  {t('projectCopilot.reloadConfig')}
                </Button>
              </div>
            ) : null}

            {configQuery.isError ? (
              <div
                role="alert"
                className="mt-3 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                <span>
                  {getApiErrorMessage(configQuery.error, t('projectCopilot.configFailed'))}
                </span>
                <Button size="sm" variant="outline" onClick={() => void configQuery.refetch()}>
                  {t('common.retry')}
                </Button>
              </div>
            ) : null}

            {configQuery.isPending && open ? (
              <p className="mt-2 text-xs text-muted-foreground" role="status">
                {t('projectCopilot.loadingConfig')}
              </p>
            ) : null}

            {projectQuery.isError || membersQuery.isError ? (
              <div
                role="alert"
                className="mt-3 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                <span>{t('projectCopilot.projectAccessFailed')}</span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    void projectQuery.refetch()
                    void membersQuery.refetch()
                  }}
                >
                  {t('common.retry')}
                </Button>
              </div>
            ) : null}

            {!canManage && config?.agent_id === null ? (
              <p className="mt-2 text-xs text-muted-foreground">
                {t('projectCopilot.ownerAdminRequired')}
              </p>
            ) : null}
            {membersQuery.isSuccess && !canWrite ? (
              <p className="mt-2 rounded-md bg-muted/50 p-2 text-xs text-muted-foreground">
                {t('projectCopilot.readOnlyAccess')}
              </p>
            ) : null}
            {membersQuery.isPending && open ? (
              <p className="mt-2 text-xs text-muted-foreground" role="status">
                {t('projectCopilot.checkingAccess')}
              </p>
            ) : null}
          </div>

          <div ref={messageListRef} className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-5">
            {taskLoading ? (
              <p
                className="mb-3 rounded-md bg-muted/50 p-3 text-xs text-muted-foreground"
                role="status"
              >
                {t('projectCopilot.loadingTask')}
              </p>
            ) : null}
            {taskError ? (
              <div
                role="alert"
                className="mb-3 flex items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs text-destructive"
              >
                <span>{getApiErrorMessage(taskError, t('projectCopilot.taskContextFailed'))}</span>
                <Button size="sm" variant="outline" onClick={retryTask}>
                  {t('common.retry')}
                </Button>
              </div>
            ) : null}

            {task ? (
              <div className="mb-4 rounded-lg border border-primary/20 bg-primary/5 p-3">
                <div className="flex items-center gap-2 text-xs font-semibold text-primary">
                  <ChatCircleDots size={14} />
                  {t('projectCopilot.currentTask')}
                </div>
                <p className="mt-1 truncate text-sm font-medium" title={task.title}>
                  {task.title}
                </p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {t('projectCopilot.currentTaskHint')}
                </p>
                <Button
                  className="mt-3 w-full sm:w-auto"
                  size="sm"
                  variant="outline"
                  disabled={!taskContextReady || !config?.agent_id || !canWrite || hasPendingWork}
                  onClick={organizeTask}
                >
                  <Sparkle size={14} />
                  {t('projectCopilot.organizeTask')}
                </Button>
                <p className="mt-2 text-[11px] text-muted-foreground">
                  {t('projectCopilot.organizeTaskHint')}
                </p>
              </div>
            ) : null}

            {run && isProjectCopilotRunPending(run.status) ? (
              <div
                className="mb-4 flex items-center gap-2 rounded-md bg-muted/60 px-3 py-2 text-xs text-muted-foreground"
                role="status"
              >
                <CircleNotch size={14} className="animate-spin" aria-hidden="true" />
                <span>
                  {t('projectCopilot.runWorking', { status: runStatusLabel(t, run.status) })}
                </span>
              </div>
            ) : null}

            {runQuery.isError ? (
              <div
                role="alert"
                className="mb-4 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs text-destructive"
              >
                <span>
                  {getApiErrorMessage(runQuery.error, t('projectCopilot.runStatusFailed'))}
                </span>
                <Button size="sm" variant="outline" onClick={() => void runQuery.refetch()}>
                  {t('projectCopilot.retryStatus')}
                </Button>
              </div>
            ) : null}

            {cancelRun.isError ? (
              <div
                role="alert"
                className="mb-4 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs text-destructive"
              >
                <span>
                  {getApiErrorMessage(cancelRun.error, t('projectCopilot.cancelTurnFailed'))}
                </span>
                <Button size="sm" variant="outline" onClick={() => cancelActiveRun()}>
                  {t('projectCopilot.retryCancel')}
                </Button>
              </div>
            ) : null}

            {messagesQuery.isError && !messagesQuery.data ? (
              <div
                role="alert"
                className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs text-destructive"
              >
                <span>
                  {getApiErrorMessage(messagesQuery.error, t('projectCopilot.messagesFailed'))}
                </span>
                <Button size="sm" variant="outline" onClick={() => void messagesQuery.refetch()}>
                  {t('common.retry')}
                </Button>
              </div>
            ) : null}

            {messagesQuery.isPending && open ? (
              <div
                className="flex items-center gap-2 py-6 text-sm text-muted-foreground"
                role="status"
              >
                <CircleNotch size={16} className="animate-spin" />
                {t('projectCopilot.loadingMessages')}
              </div>
            ) : null}

            {messagesQuery.data ? (
              <div
                role="log"
                aria-label={t('projectCopilot.conversation')}
                aria-live="polite"
                className="space-y-4"
              >
                {messagesQuery.hasNextPage ? (
                  <div className="text-center">
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={messagesQuery.isFetchingNextPage}
                      onClick={loadOlderMessages}
                    >
                      {messagesQuery.isFetchingNextPage
                        ? t('projectCopilot.loadingOlder')
                        : t('projectCopilot.loadOlder')}
                    </Button>
                  </div>
                ) : null}
                {messages.map((message) => (
                  <article
                    key={message.id}
                    className={cn(
                      'max-w-[92%] rounded-xl px-3 py-2.5 text-sm',
                      message.role === 'user'
                        ? 'ml-auto bg-primary text-primary-foreground'
                        : 'mr-auto border bg-card',
                    )}
                  >
                    <div className="mb-1 text-[10px] font-semibold uppercase tracking-wide opacity-70">
                      <span title={t('projectCopilot.actorId', { id: message.actor_id })}>
                        {messageActorLabel(message)}
                      </span>
                    </div>
                    {message.role === 'assistant' ? (
                      <MarkdownView content={message.content} className="text-sm" />
                    ) : (
                      <p className="whitespace-pre-wrap break-words">{message.content}</p>
                    )}
                    <time
                      className="mt-2 block text-right text-[10px] opacity-60"
                      dateTime={message.created_at}
                    >
                      {new Date(message.created_at).toLocaleTimeString([], {
                        hour: '2-digit',
                        minute: '2-digit',
                      })}
                    </time>
                  </article>
                ))}
                {messages.length === 0 ? (
                  <div className="mx-auto max-w-sm py-10 text-center">
                    <Robot size={28} className="mx-auto text-muted-foreground" />
                    <p className="mt-3 text-sm font-medium">{t('projectCopilot.emptyTitle')}</p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {t('projectCopilot.emptyDescription')}
                    </p>
                  </div>
                ) : null}
              </div>
            ) : null}

            <details className="mt-5 border-t pt-3">
              <summary className="cursor-pointer select-none text-xs font-semibold text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring">
                {t('projectCopilot.activity.title')}
                {activityItems.length ? ` · ${activityItems.length}` : ''}
              </summary>
              <div className="mt-3 space-y-2">
                {activityQuery.isPending ? (
                  <p className="text-xs text-muted-foreground" role="status">
                    {t('projectCopilot.activity.loading')}
                  </p>
                ) : null}
                {activityQuery.isError ? (
                  <div
                    role="alert"
                    className="flex flex-wrap items-center justify-between gap-2 text-xs text-destructive"
                  >
                    <span>{t('projectCopilot.activity.failed')}</span>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => void activityQuery.refetch()}
                    >
                      {t('common.retry')}
                    </Button>
                  </div>
                ) : null}
                {activityItems.length === 0 &&
                !activityQuery.isPending &&
                !activityQuery.isError ? (
                  <p className="text-xs text-muted-foreground">
                    {t('projectCopilot.activity.empty')}
                  </p>
                ) : null}
                {activityItems.map((item) => {
                  const isWorkUnit =
                    item.entity_type === 'work_unit' || item.entity_type === 'work_unit_dependency'
                  const shortId = item.entity_id.slice(0, 8)
                  return (
                    <article key={item.id} className="rounded-md bg-muted/40 px-3 py-2 text-xs">
                      <p
                        className="font-medium"
                        title={t('projectCopilot.activity.actorId', { id: item.actor_id })}
                      >
                        {activityLabel(item)}
                      </p>
                      <div className="mt-1 flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-[10px] text-muted-foreground">
                        <div className="flex flex-wrap items-center gap-2">
                          <span title={t('projectCopilot.activity.actorId', { id: item.actor_id })}>
                            {t('projectCopilot.copilotActor')}
                          </span>
                          <span>
                            {t('projectCopilot.activity.requestedBy', {
                              name: activityRequesterName(item.requested_by_user_id),
                            })}
                          </span>
                        </div>
                        <div className="flex flex-wrap items-center gap-2">
                          {isWorkUnit && item.task_id ? (
                            <Link
                              to="/tasks/$taskId/$tab"
                              params={{ taskId: item.task_id, tab: 'work' }}
                              className="text-primary underline-offset-2 hover:underline"
                              title={t('projectCopilot.activity.openWorkUnit', {
                                id: item.entity_id,
                              })}
                            >
                              {t('projectCopilot.activity.workUnitLink', { id: shortId })}
                            </Link>
                          ) : item.task_id ? (
                            <Link
                              to="/tasks/$taskId/$tab"
                              params={{ taskId: item.task_id, tab: 'work' }}
                              className="text-primary underline-offset-2 hover:underline"
                            >
                              {t('projectCopilot.activity.openTask')}
                            </Link>
                          ) : null}
                          <time dateTime={item.created_at}>
                            {new Date(item.created_at).toLocaleString([], {
                              dateStyle: 'short',
                              timeStyle: 'short',
                            })}
                          </time>
                        </div>
                      </div>
                    </article>
                  )
                })}
                {activityQuery.hasNextPage ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={activityQuery.isFetchingNextPage}
                    onClick={() => void activityQuery.fetchNextPage()}
                  >
                    {activityQuery.isFetchingNextPage
                      ? t('projectCopilot.activity.loading')
                      : t('projectCopilot.activity.loadOlder')}
                  </Button>
                ) : null}
              </div>
            </details>

            <details
              className="mt-3 border-t pt-3"
              onToggle={(event) => setTraceOpen(event.currentTarget.open)}
            >
              <summary className="cursor-pointer select-none text-xs font-semibold text-muted-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring">
                {t('projectCopilot.trace.title')}
                {traceItems.length ? ` · ${traceItems.length}` : ''}
              </summary>
              <p className="mt-2 text-[11px] text-muted-foreground">
                {t('projectCopilot.trace.hint')}
              </p>
              {traceQuery.isPending ? (
                <p className="mt-2 text-xs text-muted-foreground" role="status">
                  {t('projectCopilot.trace.loading')}
                </p>
              ) : null}
              {traceQuery.isError ? (
                <div className="mt-2 flex flex-wrap items-center justify-between gap-2 text-xs text-destructive">
                  <span>{t('projectCopilot.trace.failed')}</span>
                  <Button size="sm" variant="outline" onClick={() => void traceQuery.refetch()}>
                    {t('common.retry')}
                  </Button>
                </div>
              ) : null}
              {!traceQuery.isPending && !traceQuery.isError && traceItems.length === 0 ? (
                <p className="mt-2 text-xs text-muted-foreground">
                  {t('projectCopilot.trace.empty')}
                </p>
              ) : null}
              <div className="mt-2 max-h-72 space-y-1 overflow-y-auto">
                {traceItems.map((entry) => (
                  <div
                    key={`${entry.sequence}-${entry.kind}`}
                    className="rounded-md bg-muted/40 px-2 py-1 text-[11px]"
                  >
                    <div className="flex flex-wrap items-center gap-2 text-[10px] text-muted-foreground">
                      <span className="font-mono uppercase">{traceEntryLabel(entry)}</span>
                      <time dateTime={entry.timestamp}>
                        {new Date(entry.timestamp).toLocaleTimeString([], {
                          hour: '2-digit',
                          minute: '2-digit',
                          second: '2-digit',
                        })}
                      </time>
                    </div>
                    <p className="whitespace-pre-wrap break-words">{traceEntryText(entry)}</p>
                  </div>
                ))}
              </div>
            </details>
          </div>

          <div className="shrink-0 border-t bg-background px-4 py-3 sm:px-5">
            {showDraftContextNotice ? (
              <div className="mb-2 flex flex-wrap items-center justify-between gap-2 rounded-md bg-muted/60 p-2 text-xs">
                <span className="text-muted-foreground">
                  {draftTaskContext
                    ? t('projectCopilot.draftLinkedTo', { title: draftTaskContext.title })
                    : t('projectCopilot.draftNotLinked')}
                </span>
                {task ? (
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => setDraftTaskContext({ id: task.id, title: task.title })}
                  >
                    {t('projectCopilot.useCurrentTask')}
                  </Button>
                ) : draftTaskContext ? (
                  <Button size="sm" variant="outline" onClick={() => setDraftTaskContext(null)}>
                    {t('projectCopilot.removeTaskContext')}
                  </Button>
                ) : null}
              </div>
            ) : null}

            {!config?.agent_id && config && canManage ? (
              <p className="mb-2 text-xs text-muted-foreground">
                {t('projectCopilot.assignAgentToChat')}
              </p>
            ) : null}

            {sendMessage.isPending || (activeRunId && !run) ? (
              <div
                role="status"
                className="mb-2 flex items-center gap-2 text-xs text-muted-foreground"
              >
                <CircleNotch size={14} className="animate-spin" aria-hidden="true" />
                <span>{t('projectCopilot.sending')}</span>
              </div>
            ) : null}

            {terminalRunDetail &&
            ['failed', 'interrupted', 'cancelled'].includes(terminalRunDetail.run.status) ? (
              <div
                role="alert"
                className="mb-2 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                <span>
                  {t('projectCopilot.runEnded', {
                    status: runStatusLabel(t, terminalRunDetail.run.status),
                  })}
                  {runFailureMessage ? ` ${runFailureMessage}` : null}
                </span>
              </div>
            ) : null}

            {sendError ? (
              <div
                role="alert"
                className="mb-2 flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-destructive"
              >
                <span>{getApiErrorMessage(sendError, t('projectCopilot.sendFailed'))}</span>
                {retryPayload ? (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={!canWrite || sendMessage.isPending || runIsPending}
                    onClick={() => send(retryPayload)}
                  >
                    <ArrowClockwise size={13} />
                    {t('projectCopilot.retrySameMessage')}
                  </Button>
                ) : null}
              </div>
            ) : null}

            {messagesQuery.isError && messagesQuery.data ? (
              <div
                role="alert"
                className="mb-2 flex items-center justify-between gap-2 text-xs text-destructive"
              >
                <span>{t('projectCopilot.messagesRefreshFailed')}</span>
                <Button size="sm" variant="outline" onClick={() => void messagesQuery.refetch()}>
                  {t('common.retry')}
                </Button>
              </div>
            ) : null}

            {run && isProjectCopilotRunPending(run.status) ? (
              <div className="mb-2 flex items-center justify-between gap-2 text-xs text-muted-foreground">
                <span>{t('projectCopilot.waitForRun')}</span>
                <div className="flex items-center gap-2">
                  {runQuery.isError ? null : <span>{runStatusLabel(t, run.status)}</span>}
                  {canWrite ? (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={cancelRun.isPending}
                      onClick={cancelActiveRun}
                    >
                      {cancelRun.isPending
                        ? t('projectCopilot.stoppingTurn')
                        : t('projectCopilot.stopTurn')}
                    </Button>
                  ) : null}
                </div>
              </div>
            ) : null}
            {run && isProjectCopilotRunPending(run.status) ? (
              <p className="mb-2 text-[10px] text-muted-foreground">
                {t('projectCopilot.stopTurnHint')}
              </p>
            ) : null}

            <form
              className="flex items-end gap-2"
              onSubmit={(event) => {
                event.preventDefault()
                submitDraft()
              }}
            >
              <Textarea
                aria-label={t('projectCopilot.messageLabel')}
                value={draft}
                onChange={(event) => {
                  const value = event.currentTarget.value
                  if (!draft.trim() && value.trim()) {
                    setDraftTaskContext(task ? { id: task.id, title: task.title } : null)
                  }
                  setDraft(value)
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                    event.preventDefault()
                    submitDraft()
                  }
                }}
                placeholder={t('projectCopilot.placeholder')}
                rows={2}
                className="min-h-[72px] max-h-36 resize-y"
                disabled={
                  !taskContextReady ||
                  !config?.agent_id ||
                  !canWrite ||
                  hasPendingWork ||
                  sendMessage.isPending
                }
              />
              <Button
                type="submit"
                size="icon"
                aria-label={t('projectCopilot.send')}
                disabled={
                  !draft.trim() ||
                  !config?.agent_id ||
                  !canWrite ||
                  !taskContextReady ||
                  hasPendingWork ||
                  sendMessage.isPending ||
                  Boolean(retryPayload)
                }
              >
                {sendMessage.isPending ? (
                  <CircleNotch size={16} className="animate-spin" />
                ) : (
                  <PaperPlaneRight size={16} />
                )}
              </Button>
            </form>
            <p className="mt-1.5 text-[10px] text-muted-foreground">
              {draftTaskContext
                ? t('projectCopilot.draftTaskContext', { title: draftTaskContext.title })
                : t('projectCopilot.projectContextOnly')}
            </p>
          </div>
        </div>
      </Dialog>
    </>
  )
}

function AgentAssignmentEditor({
  projectId,
  ownerId,
  supportedExecutorTypes,
  supportedDaemonId,
  value,
  currentAgentId,
  canManage,
  isSaving,
  onChange,
  onSave,
  onCancel,
}: {
  projectId: string
  ownerId: string | null
  supportedExecutorTypes: string[]
  supportedDaemonId: string | null
  value: string | null
  currentAgentId: string | null
  canManage: boolean
  isSaving: boolean
  onChange: (agentId: string | null) => void
  onSave: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const agentsQuery = useAgentsInfiniteQuery(100)
  const agents = useMemo(
    () => agentsQuery.data?.pages.flatMap((page) => page.items) ?? [],
    [agentsQuery.data],
  )
  const eligibleAgents = agents.filter(
    (agent) =>
      ownerId !== null &&
      agent.owner_id === ownerId &&
      supportedDaemonId !== null &&
      (agent.daemon_id === null || agent.daemon_id === supportedDaemonId) &&
      supportedExecutorTypes.includes(agent.executor_type),
  )
  const selectedIsEligible = value === null || eligibleAgents.some((agent) => agent.id === value)
  const currentIsEligible =
    currentAgentId === null || eligibleAgents.some((agent) => agent.id === currentAgentId)
  const displayValue = eligibleAgents.some((agent) => agent.id === value) ? value : null
  const isLoading = agentsQuery.isPending || agentsQuery.isFetchingNextPage

  useEffect(() => {
    if (!canManage || agentsQuery.isPending || value !== null || eligibleAgents.length !== 1) return
    const onlyAgent = eligibleAgents[0]
    if (onlyAgent) onChange(onlyAgent.id)
  }, [agentsQuery.isPending, canManage, eligibleAgents, onChange, value])

  return (
    <div className="mt-3 space-y-2 rounded-md bg-muted/40 p-3">
      <AgentSelector
        id={`project-copilot-agent-${projectId}`}
        agents={eligibleAgents}
        value={displayValue}
        disabled={!canManage || isSaving}
        isLoading={agentsQuery.isPending}
        label={t('projectCopilot.agentLabel')}
        placeholder={t('common.selectAgent')}
        loadingPlaceholder={t('projectCopilot.loadingAgents')}
        onChange={onChange}
      />
      <p className="text-[11px] text-muted-foreground">{t('projectCopilot.agentEligibility')}</p>
      {ownerId === null ? (
        <p className="text-[11px] text-muted-foreground">{t('projectCopilot.ownerlessProject')}</p>
      ) : null}
      {agentsQuery.isError ? (
        <div
          role="alert"
          className="flex items-center justify-between gap-2 text-xs text-destructive"
        >
          <span>{t('projectCopilot.agentsFailed')}</span>
          <Button size="sm" variant="outline" onClick={() => void agentsQuery.refetch()}>
            {t('common.retry')}
          </Button>
        </div>
      ) : null}
      {!agentsQuery.isPending && eligibleAgents.length === 0 && !agentsQuery.hasNextPage ? (
        <div className="space-y-1">
          <p className="text-[11px] text-muted-foreground">
            {t('projectCopilot.noEligibleAgents')}
          </p>
          <Link
            to="/agents"
            className="inline-flex text-[11px] font-medium text-primary underline-offset-4 hover:underline"
          >
            {t('projectCopilot.openAgentSettings')}
          </Link>
        </div>
      ) : null}
      {agentsQuery.hasNextPage ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={isLoading}
          onClick={() => void agentsQuery.fetchNextPage()}
        >
          {isLoading ? t('projectCopilot.loadingAgents') : t('projectCopilot.loadMoreAgents')}
        </Button>
      ) : null}
      {!agentsQuery.isPending && !currentIsEligible ? (
        <p role="status" className="text-[11px] text-amber-700 dark:text-amber-400">
          {t('projectCopilot.currentAgentUnsupported')}
        </p>
      ) : null}
      {currentAgentId !== null ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={!canManage || isSaving}
          onClick={() => onChange(null)}
        >
          {t('projectCopilot.clearAssignment')}
        </Button>
      ) : null}
      <div className="flex justify-end gap-2 pt-1">
        <Button size="sm" variant="ghost" disabled={isSaving} onClick={onCancel}>
          {t('common.cancel')}
        </Button>
        <Button
          size="sm"
          disabled={
            !canManage ||
            isSaving ||
            !selectedIsEligible ||
            value === currentAgentId ||
            agentsQuery.isPending
          }
          onClick={onSave}
        >
          {isSaving ? t('projectCopilot.savingAgent') : t('common.save')}
        </Button>
      </div>
    </div>
  )
}
