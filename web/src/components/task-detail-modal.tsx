import { useEffect, useMemo, useState, type KeyboardEvent } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { toast } from 'sonner'
import { ApiError } from '@/api/client'
import { ErrorBanner } from '@/components/error-banner'
import { cn } from '@/lib/cn'
import {
  useAgentsQuery,
  useApproveGate,
  useCancelTask,
  useExecutionsQuery,
  useRejectGate,
  useResetTaskWorkspace,
  useReviewsQuery,
  useValidationsQuery,
  useRecoverTask,
  useTaskQuery,
  useTransitionTask,
  useUpdateTask,
  useWorkflowQuery,
} from '@/api/hooks'
import { TaskDetailHeader } from '@/components/task-detail/task-detail-header'
import { TaskDetailSidebar } from '@/components/task-detail/task-detail-sidebar'
import {
  TaskExecutionsPanel,
} from '@/components/task-detail/task-executions-panel'
import { TaskPrSummaryCard } from '@/components/task-detail/task-pr-summary-card'
import { TaskSubtasksPanel } from '@/components/task-detail/task-subtasks-panel'
import { TaskDependenciesPanel } from '@/components/task-detail/task-dependencies-panel'
import { TaskBlockingBanner } from '@/components/task-detail/task-blocking-banner'
import { WorkflowExceptionPanel } from '@/components/task-detail/workflow-exception-panel'
import { TaskHistoryPanel } from '@/components/task-detail/task-history-panel'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { MarkdownEditor, MarkdownView } from '@/components/ui/markdown-editor'
import { getAvailableTaskTransitions } from '@/components/task-controls'
import { getApiErrorMessage } from '@/lib/api-error'
import { workflowTriggerTargets } from '@/lib/workflow-utils'
import { getBlockingAnnotation } from '@/lib/workflow-utils'
import { productTerm } from '@/lib/i18n'
import type { ReviewExecutionResponse, ValidationRunResponse, WorkflowExceptionAction } from '@/types/generated'

function formatDate(value?: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString()
}

function getErrorInfo(
  task: { error_annotation?: unknown } | undefined,
): { tone: 'timeout' | 'crash' | 'workspace'; message: string } | undefined {
  if (!task) return undefined
  const annotation = task.error_annotation as
    | { blocking_reason?: unknown; kind?: unknown; type?: unknown; message?: unknown }
    | undefined
  if (!annotation) return undefined
  if (typeof annotation.blocking_reason === 'string' && annotation.blocking_reason) return undefined
  const type = typeof annotation.type === 'string' ? annotation.type : undefined
  if (type === 'workspace_reset_required' || type === 'workspace_error') {
    const message =
      typeof annotation.message === 'string' ? annotation.message : 'Workspace is unavailable'
    return { tone: 'workspace', message }
  }
  const kind = typeof annotation.kind === 'string' ? annotation.kind : undefined
  const message = typeof annotation.message === 'string' ? annotation.message : undefined
  if (!kind && !message) return undefined
  const tone = kind === 'crash' ? 'crash' : 'timeout'
  return { tone, message: message ?? (tone === 'crash' ? 'Task crashed' : 'Task timed out') }
}

interface TaskDetailModalProps {
  taskId: string
  open: boolean
  onClose: () => void
}

export function TaskDetailModal({ taskId, open, onClose }: TaskDetailModalProps) {
  const navigate = useNavigate()
  const taskQuery = useTaskQuery(taskId)
  const executionsQuery = useExecutionsQuery(taskId)
  const reviewsQuery = useReviewsQuery(taskId)
  const validationsQuery = useValidationsQuery(taskId)
  const agentsQuery = useAgentsQuery()
  const updateTask = useUpdateTask()
  const transitionTask = useTransitionTask()
  const approveGate = useApproveGate()
  const rejectGate = useRejectGate()
  const cancelTask = useCancelTask()
  const recoverTask = useRecoverTask()

  const [editingTitle, setEditingTitle] = useState(false)
  const [titleDraft, setTitleDraft] = useState('')
  const [editingDescription, setEditingDescription] = useState(false)
  const [descriptionDraft, setDescriptionDraft] = useState('')
  const [priorityDraft, setPriorityDraft] = useState('')

  const task = taskQuery.data
  const errorInfo = useMemo(() => getErrorInfo(task), [task])
  const workflowQuery = useWorkflowQuery(task?.project_id ?? '')
  const workflow = workflowQuery.data
  const executions = useMemo(() => executionsQuery.data?.items ?? [], [executionsQuery.data])
  const agents = agentsQuery.data?.items ?? []
  const agentNamesById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent.name])),
    [agents],
  )
  const agentName = (agentId?: string | null) =>
    (agentId ? agentNamesById.get(agentId) : undefined) ?? agentId
  const reviews = reviewsQuery.data ?? []
  const validations = validationsQuery.data ?? []
  const recentReviewProjection = [...reviews].sort((a, b) =>
    b.execution.created_at.localeCompare(a.execution.created_at),
  )[0]

  useEffect(() => {
    if (!task) return
    const timeout = window.setTimeout(() => {
      setTitleDraft(task.title)
      setDescriptionDraft(task.description ?? '')
      setPriorityDraft(String(task.priority))
    }, 0)
    return () => window.clearTimeout(timeout)
  }, [task])

  useEffect(() => {
    if (open) return
    const timeout = window.setTimeout(() => {
      setEditingTitle(false)
      setEditingDescription(false)
    }, 0)
    return () => window.clearTimeout(timeout)
  }, [open])

  useEffect(() => {
    if (!open) return
    const handleKeyDown = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape' && !editingTitle && !editingDescription) {
        onClose()
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [open, onClose, editingTitle, editingDescription])

  const saveTitle = () => {
    if (!task) return
    const title = titleDraft.trim()
    if (!title || title === task.title) {
      setTitleDraft(task.title)
      setEditingTitle(false)
      return
    }
    updateTask.mutate(
      { taskId: task.id, body: { title, version: task.version } },
      { onSuccess: () => setEditingTitle(false) },
    )
  }

  const saveDescription = () => {
    if (!task) return
    const description = descriptionDraft.trim()
    const normalizedCurrent = (task.description ?? '').trim()
    if (description === normalizedCurrent) {
      setDescriptionDraft(task.description ?? '')
      setEditingDescription(false)
      return
    }
    updateTask.mutate(
      {
        taskId: task.id,
        body: {
          description: description.length > 0 ? description : null,
          version: task.version,
        },
      },
      { onSuccess: () => setEditingDescription(false) },
    )
  }

  const savePriority = () => {
    if (!task) return
    const priority = Number(priorityDraft)
    if (!Number.isFinite(priority) || priority === task.priority) {
      setPriorityDraft(String(task.priority))
      return
    }
    updateTask.mutate({ taskId: task.id, body: { priority, version: task.version } })
  }

  const onTitleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Enter') {
      event.preventDefault()
      saveTitle()
    }
    if (event.key === 'Escape') {
      setTitleDraft(task?.title ?? '')
      setEditingTitle(false)
    }
  }

  const onStatusChange = (status: string, reason?: string) => {
    if (!task || status === task.status) return
    transitionTask.mutate(
      {
        taskId: task.id,
        body: { status, version: task.version, reason },
        currentStatus: task.status,
      },
      {
        onError: (error) => {
          if (error instanceof ApiError && error.status === 412) {
            let msg = 'Transition blocked by guard condition'
            try {
              const b = JSON.parse(error.message) as { message?: string }
              if (b.message) msg = b.message
            } catch {
              /* raw */
            }
            toast.error(msg)
            return
          }
          toast.error(getApiErrorMessage(error, 'Transition failed'))
        },
      },
    )
  }

  const onRecoverTask = (
    action: Parameters<typeof recoverTask.mutate>[0]['action'],
    input?: { reason?: string; context?: string },
  ) => {
    if (!task) return
    recoverTask.mutate(
      { taskId: task.id, action, reason: input?.reason, context: input?.context },
      {
        onError: (error) => toast.error(getApiErrorMessage(error, 'Task recovery failed')),
      },
    )
  }

  const onOpenWorkflowExceptionAction = (action: WorkflowExceptionAction) => {
    if (!task) return
    onClose()
    if (action.target_execution_id) {
      void navigate({
        to: '/tasks/$taskId/executions/$executionId',
        params: { taskId: task.id, executionId: action.target_execution_id },
        search: { followUp: true },
      })
      return
    }
    void navigate({ to: '/tasks/$taskId', params: { taskId: task.id } })
  }

  const onCancelTask = () => {
    if (!task) return
    cancelTask.mutate(task.id, {
      onError: (error) => toast.error(getApiErrorMessage(error, 'Task cancellation failed')),
    })
  }

  const onApproveGate = (stateName: string) => {
    if (!task) return
    const blockingAnnotation = getBlockingAnnotation(task)
    if (
      task.status === 'blocked' &&
      stateName === 'blocked' &&
      blockingAnnotation?.recovery_actions?.includes('retry_hook')
    ) {
      onRecoverTask('retry_hook')
      return
    }
    approveGate.mutate(
      { taskId: task.id, stateName, body: { version: task.version } },
      { onError: (error) => toast.error(getApiErrorMessage(error, 'Gate approval failed')) },
    )
  }

  const onRejectGate = (stateName: string, reason: string) => {
    if (!task) return
    rejectGate.mutate(
      { taskId: task.id, stateName, body: { version: task.version, reason } },
      { onError: (error) => toast.error(getApiErrorMessage(error, 'Gate rejection failed')) },
    )
  }

  const openFullPage = () => {
    onClose()
    void navigate({ to: '/tasks/$taskId', params: { taskId } })
  }

  const showReviewTab =
    task?.status === 'review' ||
    Boolean(task && task.role_assignments.some((r) => r.role_name === 'reviewer'))
  const cancellationState = workflow?.cancellation_state ?? 'cancelled'
  const terminal = task?.status === 'done' || task?.status === cancellationState
  const hiddenTransitions = [cancellationState, 'merging']
  const availableTransitions = (
    workflow && task
      ? workflowTriggerTargets(workflow, task.status)
      : task
        ? getAvailableTaskTransitions(task.status)
        : []
  ).filter((s) => !hiddenTransitions.includes(s))

  if (!open) return null

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[5vh]">
      <div className="absolute inset-0 bg-black/50 backdrop-blur-[2px]" onClick={onClose} />

      <div
        aria-label={task?.title ?? 'Task detail'}
        aria-modal="true"
        role="dialog"
        className="relative z-10 flex max-h-[90vh] w-full max-w-5xl flex-col overflow-hidden rounded-xl border bg-background shadow-float animate-slide-in"
      >
        <TaskDetailHeader
          task={task}
          editingTitle={editingTitle}
          titleDraft={titleDraft}
          updatePending={updateTask.isPending}
          onCancelTitle={() => {
            setTitleDraft(task?.title ?? '')
            setEditingTitle(false)
          }}
          onClose={onClose}
          onEditTitle={() => setEditingTitle(true)}
          onOpenFullPage={openFullPage}
          onSaveTitle={saveTitle}
          onTitleChange={setTitleDraft}
          onTitleKeyDown={onTitleKeyDown}
        />

        <div className="flex min-h-0 flex-1 overflow-hidden">
          <main className="flex-1 overflow-y-auto p-6">
            {taskQuery.isLoading ? (
              <div className="space-y-4">
                <Skeleton className="h-8 w-3/4" />
                <Skeleton className="h-24 w-full" />
              </div>
            ) : taskQuery.isError ? (
              <ErrorBanner
                error={taskQuery.error}
                fallback="Task failed to load"
                onRetry={() => void taskQuery.refetch()}
              />
            ) : task ? (
              <div className="space-y-6">
                {errorInfo && !task.workflow_exception ? (
                  errorInfo.tone === 'workspace' ? (
                    <ModalWorkspaceErrorBanner
                      taskId={taskId}
                      errorInfo={errorInfo}
                      needsReset={task.error_annotation?.type === 'workspace_reset_required'}
                    />
                  ) : (
                    <div
                      className={cn(
                        'rounded-lg border p-3 text-sm',
                        errorInfo.tone === 'crash'
                          ? 'border-red-200 bg-red-50 text-red-800 dark:border-red-800 dark:bg-red-950 dark:text-red-300'
                          : 'border-orange-200 bg-orange-50 text-orange-800 dark:border-orange-800 dark:bg-orange-950 dark:text-orange-300',
                      )}
                    >
                      {errorInfo.message}
                    </div>
                  )
                ) : null}

                <WorkflowExceptionPanel
                  task={task}
                  actions={task.workflow_exception?.actions ?? []}
                  recoverPending={recoverTask.isPending}
                  terminal={terminal}
                  cancelPending={cancelTask.isPending}
                  onRecover={onRecoverTask}
                  onOpenInteractive={onOpenWorkflowExceptionAction}
                  onCancelTask={onCancelTask}
                />
                {!task.workflow_exception ? <TaskBlockingBanner task={task} /> : null}

                <TaskPrSummaryCard task={task} />

                <div>
                  <p className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    Description
                  </p>
                  {editingDescription ? (
                    <div className="space-y-2">
                      <MarkdownEditor
                        autoFocus
                        minHeight="128px"
                        value={descriptionDraft}
                        onChange={setDescriptionDraft}
                        onKeyDown={(e) => {
                          if (e.key === 'Escape') {
                            setDescriptionDraft(task.description ?? '')
                            setEditingDescription(false)
                          }
                        }}
                      />
                      <div className="flex items-center gap-2">
                        <Button
                          size="sm"
                          disabled={updateTask.isPending}
                          onClick={saveDescription}
                        >
                          Save
                        </Button>
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => {
                            setDescriptionDraft(task.description ?? '')
                            setEditingDescription(false)
                          }}
                        >
                          Cancel
                        </Button>
                      </div>
                    </div>
                  ) : task.description?.trim() ? (
                    <button
                      className="block w-full cursor-pointer rounded-lg border border-dashed p-3 text-left transition-colors hover:border-border hover:bg-accent"
                      type="button"
                      onClick={() => setEditingDescription(true)}
                    >
                      <MarkdownView content={task.description} />
                    </button>
                  ) : (
                    <button
                      className="block w-full cursor-pointer rounded-lg border border-dashed p-3 text-left text-sm text-muted-foreground transition-colors hover:border-border hover:bg-accent"
                      type="button"
                      onClick={() => setEditingDescription(true)}
                    >
                      <span className="italic opacity-60">Click to add a description…</span>
                    </button>
                  )}
                </div>

                {task.parent_task_id == null ? <TaskSubtasksPanel task={task} /> : null}

                <TaskDependenciesPanel task={task} />

                <Tabs defaultValue="executions">
                  <TabsList>
                    <TabsTrigger value="executions">
                      {productTerm('run', 0)}
                      {executions.length > 0 && (
                        <span className="ml-1.5 inline-flex h-5 min-w-[20px] items-center justify-center rounded-full bg-muted-foreground/10 px-1.5 text-micro font-medium">
                          {executions.length}
                        </span>
                      )}
                    </TabsTrigger>
                    {showReviewTab ? (
                      <TabsTrigger value="review">
                        Review
                        {recentReviewProjection && (
                          <span
                            className="ml-1.5 inline-flex h-5 items-center rounded-full bg-muted px-1.5 text-micro font-medium"
                          >
                            {recentReviewProjection.execution.status}
                          </span>
                        )}
                      </TabsTrigger>
                    ) : null}
                    <TabsTrigger value="history">History</TabsTrigger>
                  </TabsList>

                  <TabsContent value="executions" className="space-y-2 pt-2">
                    <TaskExecutionsPanel
                      agentName={agentName}
                      executions={executions}
                      formatDate={formatDate}
                      isLoading={executionsQuery.isLoading}
                      taskId={taskId}
                      onClose={onClose}
                    />
                  </TabsContent>

                  {showReviewTab ? (
                    <TabsContent value="review" className="space-y-4 pt-2">
                      <ModalReviewSummary
                        reviews={reviews}
                        validations={validations}
                        reviewsLoading={reviewsQuery.isLoading}
                        validationsLoading={validationsQuery.isLoading}
                        onOpenFullPage={openFullPage}
                      />
                    </TabsContent>
                  ) : null}

                  <TabsContent value="history" className="pt-0">
                    <TaskHistoryPanel taskId={taskId} />
                  </TabsContent>
                </Tabs>
              </div>
            ) : (
              <div className="rounded-lg border border-dashed p-8 text-center text-sm text-muted-foreground">
                Task not found
              </div>
            )}
          </main>

          <TaskDetailSidebar
            agents={agents}
            agentName={agentName}
            availableTransitions={availableTransitions}
            cancelTask={cancelTask}
            executions={executions}
            formatDate={formatDate}
            isLoading={taskQuery.isLoading}
            priorityDraft={priorityDraft}
            savePriority={savePriority}
            setPriorityDraft={setPriorityDraft}
            task={task}
            transitionPending={
              transitionTask.isPending || approveGate.isPending || rejectGate.isPending
            }
            workflow={workflow}
            onApproveGate={onApproveGate}
            onRejectGate={onRejectGate}
            onStatusChange={onStatusChange}
          />
        </div>
      </div>
    </div>
  )
}

function ModalReviewSummary({
  reviews,
  validations,
  reviewsLoading,
  validationsLoading,
  onOpenFullPage,
}: {
  reviews: ReviewExecutionResponse[]
  validations: ValidationRunResponse[]
  reviewsLoading: boolean
  validationsLoading: boolean
  onOpenFullPage: () => void
}) {
  return (
    <div className="space-y-4">
      <section className="space-y-2">
        <p className="text-sm font-medium">Review Executions</p>
        {reviewsLoading ? <p className="text-sm text-muted-foreground">Loading…</p> : reviews.length === 0 ? (
          <p className="text-sm text-muted-foreground">No reviewer Execution has been recorded.</p>
        ) : (
          reviews.map(({ execution, report }) => (
            <div key={execution.id} className="rounded-md border p-3 text-xs">
              <p><span className="font-medium">Execution:</span> <code>{execution.id}</code> · {execution.status}</p>
              <p className="mt-1"><span className="font-medium">Actor:</span> {execution.actor_ref?.kind ?? 'unknown'} / {execution.actor_ref?.id ?? 'unknown'}</p>
              <p className="mt-1"><span className="font-medium">ReviewReport:</span> {report ? <code>{report.id}</code> : 'not produced'}</p>
            </div>
          ))
        )}
        <button type="button" className="cursor-pointer text-xs text-primary hover:underline" onClick={onOpenFullPage}>
          Open Review and Validation detail
        </button>
      </section>
      <section className="space-y-2">
        <p className="text-sm font-medium">ValidationRuns</p>
        {validationsLoading ? <p className="text-sm text-muted-foreground">Loading…</p> : validations.length === 0 ? (
          <p className="text-sm text-muted-foreground">No deterministic validation is recorded.</p>
        ) : (
          validations.map((run) => (
            <div key={run.id} className="rounded-md border p-3 text-xs">
              <p>{run.check_identity} · {run.status}</p>
              <p className="mt-1">ValidationRun <code>{run.id}</code> · commit <code>{run.commit_sha}</code></p>
              <p className="mt-1 break-all">Workspace snapshot <code>{run.workspace_snapshot_digest}</code></p>
              <p className="mt-1">Evidence: {run.evidence_ids.map((id) => <code key={id} className="mr-2">{id}</code>).join(' ') || 'none'}</p>
            </div>
          ))
        )}
      </section>
    </div>
  )
}

function ModalWorkspaceErrorBanner({
  taskId,
  errorInfo,
  needsReset,
}: {
  taskId: string
  errorInfo: { message: string }
  needsReset: boolean
}) {
  const resetWorkspace = useResetTaskWorkspace()
  const [confirming, setConfirming] = useState(false)

  return (
    <div className="rounded-lg border border-amber-300 bg-amber-50 p-4 dark:border-amber-700 dark:bg-amber-950">
      <div className="flex items-start gap-3">
        <div className="mt-0.5 shrink-0 text-amber-600 dark:text-amber-400">
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
        </div>
        <div className="flex-1 space-y-2">
          <p className="text-sm font-medium text-amber-800 dark:text-amber-200">
            {needsReset ? 'Workspace Reset Required' : 'Workspace Error'}
          </p>
          <p className="text-sm text-amber-700 dark:text-amber-300">{errorInfo.message}</p>
          {needsReset ? (
            confirming ? (
              <div className="flex items-center gap-2 pt-1">
                <p className="text-xs text-amber-700 dark:text-amber-300">
                  This will recreate the workspace from the default branch. Any uncommitted work
                  will be lost.
                </p>
                <Button
                  variant="destructive"
                  size="sm"
                  disabled={resetWorkspace.isPending}
                  onClick={() => {
                    resetWorkspace.mutate(taskId, {
                      onSuccess: () => {
                        toast.success('Workspace reset successfully')
                        setConfirming(false)
                      },
                      onError: (error) =>
                        toast.error(getApiErrorMessage(error, 'Reset failed')),
                    })
                  }}
                >
                  {resetWorkspace.isPending ? 'Resetting...' : 'Confirm Reset'}
                </Button>
                <Button variant="outline" size="sm" onClick={() => setConfirming(false)}>
                  Cancel
                </Button>
              </div>
            ) : (
              <Button
                variant="outline"
                size="sm"
                className="mt-1 border-amber-300 text-amber-800 hover:bg-amber-100 dark:border-amber-600 dark:text-amber-200 dark:hover:bg-amber-900"
                onClick={() => setConfirming(true)}
              >
                Reset Workspace
              </Button>
            )
          ) : null}
        </div>
      </div>
    </div>
  )
}
