import { useEffect, useState, type FormEvent } from 'react'
import { Link } from '@tanstack/react-router'
import { toast } from 'sonner'
import {
  useAgentsInfiniteQuery,
  useCreateTaskIntervention,
  useDaemonsQuery,
  useEvaluateGate,
  useExecutionsQuery,
  useLaunchTask,
  useReviewsQuery,
  useTaskDispatchQuery,
  useTaskCoordinationQuery,
  useTaskGatesQuery,
  useTaskLifecycleTransitionsQuery,
  useTaskReviewLinesQuery,
  useTaskQuery,
  useProjectQuery,
  useMembersQuery,
  useUpdateTask,
  useTaskWorkUnitsQuery,
  useTransitionTaskLifecycle,
  useValidationsQuery,
} from '@/api/hooks'
import { apiFetch } from '@/api/client'
import { TaskArtifactsPanel } from '@/components/task-detail/task-artifacts-panel'
import { ErrorBanner } from '@/components/error-banner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { MarkdownEditor, MarkdownView } from '@/components/ui/markdown-editor'
import { getApiErrorMessage } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'
import { TaskTerminalPanel } from '@/components/task-detail/task-terminal-panel'
import { TaskWorkManagement } from '@/components/task-detail/task-work-management'
import { TaskReworkPanel } from '@/components/task-detail/task-rework-panel'
import { WorkUnitHumanReviewPanel } from '@/components/task-detail/work-unit-review-panel'
import { ActorIdentity } from '@/features/task-roles/ActorIdentity'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import { TaskCoordinationPanel } from '@/components/task-detail/task-coordination-panel'
import { TaskJourneyPanel } from '@/components/task-detail/task-journey-panel'
import type {
  GateEvaluationResponse,
  MergeAfterGateResponse,
  ReviewExecutionResponse,
  Task,
  TaskDispatchActionRequest,
  TaskLifecycleState,
} from '@/types/generated'

export type TaskDetailTab =
  | 'overview'
  | 'work'
  | 'executions'
  | 'artifacts'
  | 'review'
  | 'terminal'
  | 'history'

export function isTaskDetailTab(value: string | undefined): value is TaskDetailTab {
  return (
    value === 'overview' ||
    value === 'work' ||
    value === 'executions' ||
    value === 'artifacts' ||
    value === 'review' ||
    value === 'terminal' ||
    value === 'history'
  )
}

const nextLifecycleStates: Partial<Record<TaskLifecycleState, TaskLifecycleState[]>> = {
  backlog: ['ready', 'blocked', 'cancelled'],
  ready: ['active', 'blocked', 'cancelled'],
  active: ['ready', 'blocked', 'cancelled'],
  blocked: ['ready', 'active', 'cancelled'],
  ready_to_merge: [],
  merging: [],
  done: [],
  cancelled: [],
}

const tabs: TaskDetailTab[] = [
  'overview',
  'work',
  'executions',
  'artifacts',
  'review',
  'terminal',
  'history',
]

const tabLabels: Record<TaskDetailTab, string> = {
  overview: 'Overview',
  work: 'Advanced work',
  executions: 'Activity',
  artifacts: 'Results',
  review: 'Review',
  terminal: 'Terminal',
  history: 'History',
}

const lifecycleLabels: Record<TaskLifecycleState, string> = {
  backlog: 'Backlog',
  ready: 'Ready',
  active: 'In progress',
  blocked: 'Blocked',
  ready_to_merge: 'Ready for integration',
  merging: 'Integrating',
  done: 'Done',
  cancelled: 'Cancelled',
}

function launchBlockerMessage(code: string, message: string): string {
  const labels: Record<string, string> = {
    no_work_units: 'No work items have been prepared for this Task yet.',
    work_unit_unassigned: 'A work item needs an eligible person or Agent assigned.',
    agent_unavailable: 'An assigned Agent or host is unavailable.',
    membership_inactive: 'An assigned Actor is no longer eligible for this role.',
    workspace_not_prepared:
      'A repository workspace needs to be prepared before this work can start.',
    task_blocked: 'This Task is blocked. Resolve the current block before dispatching work.',
    task_terminal: 'This Task has ended and cannot dispatch more work.',
    result_unavailable: 'A required result from earlier work is not available.',
  }
  return labels[code] ?? message
}

export function TaskDetailPage({
  taskId,
  initialTab = 'overview',
}: {
  taskId: string
  initialTab?: TaskDetailTab
}) {
  const [activeTab, setActiveTab] = useState<TaskDetailTab>(initialTab)
  const taskQuery = useTaskQuery(taskId)
  const transitionsQuery = useTaskLifecycleTransitionsQuery(taskId)
  const executionsQuery = useExecutionsQuery(taskId)
  const reviewsQuery = useReviewsQuery(taskId)
  const reviewLinesQuery = useTaskReviewLinesQuery(taskId)
  const validationsQuery = useValidationsQuery(taskId)
  const gatesQuery = useTaskGatesQuery(taskId)
  const workUnitsQuery = useTaskWorkUnitsQuery(taskId)
  const dispatchQuery = useTaskDispatchQuery(taskId)
  const coordinationQuery = useTaskCoordinationQuery(taskId)
  const transition = useTransitionTaskLifecycle()
  const updateTask = useUpdateTask()
  const evaluateGate = useEvaluateGate()
  const launchTask = useLaunchTask()
  const createIntervention = useCreateTaskIntervention()
  const userId = useAuthStore((state) => state.user?.id)
  const [evaluations, setEvaluations] = useState<Record<string, GateEvaluationResponse>>({})
  const [creatingMergeGate, setCreatingMergeGate] = useState(false)
  const [editTaskOpen, setEditTaskOpen] = useState(false)
  const [editTitle, setEditTitle] = useState('')
  const [editDescription, setEditDescription] = useState('')
  const [dispatchConfirmAction, setDispatchConfirmAction] = useState<'pause' | 'cancel' | null>(
    null,
  )

  const task = taskQuery.data
  const projectId = task?.project_id ?? ''
  const projectQuery = useProjectQuery(projectId, Boolean(projectId))
  const membersQuery = useMembersQuery(projectId, Boolean(projectId))
  const agentsQuery = useAgentsInfiniteQuery(100, Boolean(projectId))
  const daemonsQuery = useDaemonsQuery(Boolean(projectId))
  const reviews = reviewsQuery.data ?? []
  const validations = validationsQuery.data ?? []
  const executions = executionsQuery.data?.items ?? []
  const workUnits = workUnitsQuery.data ?? []
  const dispatch = dispatchQuery.data
  const hasMergeReadinessGate =
    gatesQuery.data?.some((entry) => entry.gate.gate_kind === 'merge_readiness') ?? false
  const mergeGateReviewLine =
    reviewLinesQuery.data?.find((candidate) => candidate.latest_verdict === 'pass') ??
    reviewLinesQuery.data?.[0]
  const agents = (agentsQuery.data?.pages.flatMap((page) => page.items) ?? []).filter(
    (agent) => projectQuery.data?.owner_id != null && agent.owner_id === projectQuery.data.owner_id,
  )
  const members = membersQuery.data ?? []
  const daemons = daemonsQuery.data?.items ?? []

  useEffect(() => {
    if (window.innerWidth >= 1024) return
    const activeTab = document.querySelector<HTMLElement>('[data-active-task-section="true"]')
    const navigation = activeTab?.closest<HTMLElement>('[data-task-navigation="true"]')
    if (!activeTab || !navigation) return
    const navBounds = navigation.getBoundingClientRect()
    const tabBounds = activeTab.getBoundingClientRect()
    const centeredLeft =
      navigation.scrollLeft +
      tabBounds.left -
      navBounds.left -
      (navigation.clientWidth - tabBounds.width) / 2
    if (typeof navigation.scrollTo === 'function') {
      navigation.scrollTo({ left: Math.max(0, centeredLeft), behavior: 'auto' })
    }
  }, [activeTab])

  async function transitionTo(toState: TaskLifecycleState, gateEvaluationId?: string) {
    if (!task) return
    try {
      await transition.mutateAsync({
        taskId: task.id,
        toState,
        expectedLifecycleVersion: task.lifecycle.version,
        idempotencyKey: crypto.randomUUID(),
        ...(gateEvaluationId
          ? {
              gateEvaluationId,
              reasonKind: 'gate_evaluation',
              reasonRef: gateEvaluationId,
            }
          : {}),
      })
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Task lifecycle transition failed'))
    }
  }

  function openTaskEditor() {
    if (!task) return
    setEditTitle(task.title)
    setEditDescription(task.description ?? '')
    setEditTaskOpen(true)
  }

  async function saveTask(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!task || !editTitle.trim()) return
    try {
      await updateTask.mutateAsync({
        taskId: task.id,
        body: {
          title: editTitle.trim(),
          description: editDescription.trim() || null,
          version: task.version,
        },
      })
      toast.success('Task details saved')
      setEditTaskOpen(false)
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Task details could not be saved'))
    }
  }

  async function evaluate(gateId: string) {
    try {
      const result = await evaluateGate.mutateAsync(gateId)
      setEvaluations((current) => ({ ...current, [gateId]: result }))
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Gate evaluation failed'))
    }
  }

  async function createMergeReadinessGate() {
    if (!task) return
    const line = mergeGateReviewLine
    if (!line) return
    setCreatingMergeGate(true)
    try {
      await apiFetch(`/tasks/${task.id}/gates`, {
        method: 'POST',
        body: JSON.stringify({
          gate_kind: 'merge_readiness',
          policy: {
            schema_version: 1,
            review: {
              mode: 'one_acceptable',
              required_count: 1,
              human_required: false,
              review_line_id: line.id,
            },
          },
        }),
      })
      await gatesQuery.refetch()
      toast.success('Merge-readiness check created from the exact review line')
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Merge-readiness check could not be created'))
    } finally {
      setCreatingMergeGate(false)
    }
  }

  async function merge(task: Task, evaluation: GateEvaluationResponse) {
    try {
      const result = await apiFetch<MergeAfterGateResponse>(`/tasks/${task.id}/merge`, {
        method: 'POST',
        body: JSON.stringify({ gate_evaluation_id: evaluation.id }),
      })
      toast.success(`Merge admission completed: ${result.outcome}`)
      await taskQuery.refetch()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Merge admission failed'))
    }
  }

  async function dispatchAction(action: 'launch' | 'pause' | 'resume' | 'cancel') {
    if (!task || !dispatch) return false
    const body: TaskDispatchActionRequest = {
      expected_task_version: task.version,
      expected_lifecycle_version: task.lifecycle.version,
      idempotency_key: crypto.randomUUID(),
      reason: action === 'cancel' || action === 'pause' ? 'requested from Task detail' : null,
    }
    try {
      if (action === 'launch') {
        const result = await launchTask.mutateAsync({ taskId: task.id, body })
        toast.success(
          `Task launch admitted. Executions admitted: ${result.admitted_execution_ids.length}; provider starts accepted: ${result.started_execution_ids.length}.`,
        )
      } else {
        const kind = action as 'pause' | 'resume' | 'cancel'
        const result = await createIntervention.mutateAsync({
          taskId: task.id,
          body: {
            kind,
            expected_task_version: task.version,
            expected_coordination_version: null,
            expected_lifecycle_version: task.lifecycle.version,
            instruction: null,
            plan_artifact_id: null,
            decision: null,
            rationale: null,
            reason: action === 'cancel' || action === 'pause' ? 'requested from Task detail' : null,
            brief: null,
            idempotency_key: crypto.randomUUID(),
          },
        })
        if (result.state === 'rejected') {
          throw new Error(
            result.failure_message ?? result.failure_code ?? `Task ${action} was rejected`,
          )
        }
        toast.success(
          action === 'pause'
            ? 'New work is paused; work already running may continue.'
            : action === 'resume'
              ? 'Task dispatch resumed.'
              : 'Task cancelled; Forge requested cancellation of running Executions.',
        )
      }
      await Promise.all([
        taskQuery.refetch(),
        executionsQuery.refetch(),
        workUnitsQuery.refetch(),
        dispatchQuery.refetch(),
      ])
      return true
    } catch (error) {
      toast.error(getApiErrorMessage(error, `Task ${action} failed`))
      return false
    }
  }

  if (taskQuery.isLoading) {
    return <div className="p-6 text-sm text-muted-foreground">Loading task…</div>
  }
  if (taskQuery.isError) {
    return (
      <div className="p-6">
        <ErrorBanner error={taskQuery.error} fallback="Task failed to load" />
      </div>
    )
  }
  if (!task) return null

  return (
    <main className="mx-auto w-full max-w-6xl space-y-5 p-4 sm:p-6">
      <header className="space-y-3">
        <Link
          to="/projects/$projectId/board"
          params={{ projectId: task.project_id }}
          className="text-sm text-muted-foreground hover:text-foreground"
        >
          ← Project board
        </Link>
        <div className="flex flex-wrap items-start gap-3">
          <div className="min-w-0 flex-1">
            <h1 className="break-words text-2xl font-semibold">{task.title}</h1>
            {task.description ? (
              <MarkdownView
                content={task.description}
                className="mt-2 text-sm text-muted-foreground"
              />
            ) : null}
          </div>
          <Button size="sm" variant="outline" onClick={openTaskEditor}>
            Edit task
          </Button>
          <Badge variant="outline">{lifecycleLabels[task.lifecycle.state]}</Badge>
        </div>
        <details className="text-xs text-muted-foreground">
          <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
            Task reference
          </summary>
          <div className="mt-1 flex min-w-0 items-center gap-1">
            <code className="min-w-0 break-all">{task.id}</code>
            <CopyReferenceButton value={task.id} label="Task ID" />
          </div>
        </details>
        <div className="flex flex-wrap items-center gap-2">
          {dispatch?.can_launch ? (
            <Button
              size="sm"
              disabled={launchTask.isPending}
              onClick={() => void dispatchAction('launch')}
            >
              Start ready work
            </Button>
          ) : null}
          {dispatch?.can_pause ? (
            <Button
              size="sm"
              variant="outline"
              disabled={createIntervention.isPending}
              onClick={() => setDispatchConfirmAction('pause')}
            >
              Pause new work
            </Button>
          ) : null}
          {dispatch?.can_resume ? (
            <Button
              size="sm"
              variant="outline"
              disabled={createIntervention.isPending}
              onClick={() => void dispatchAction('resume')}
            >
              Resume work
            </Button>
          ) : null}
          {dispatch?.can_cancel ? (
            <Button
              size="sm"
              variant="outline"
              disabled={createIntervention.isPending}
              onClick={() => setDispatchConfirmAction('cancel')}
            >
              Cancel Task…
            </Button>
          ) : null}
        </div>
        {dispatch?.dispatch.state === 'paused' ? (
          <p className="text-xs text-muted-foreground">
            Dispatch is paused. Work already running may continue; no new work starts until you
            resume.
          </p>
        ) : null}
        <CollapsibleSection title="Advanced Task controls" className="border-t pt-3">
          <div className="space-y-3 rounded-md bg-muted/20 p-3">
            <div>
              <h2 className="text-sm font-medium">Task lifecycle</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                Explicit state transitions are separate from dispatch and worker activity.
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
              {(nextLifecycleStates[task.lifecycle.state] ?? [])
                .filter((state) => state !== 'cancelled')
                .map((state) => (
                  <Button
                    key={state}
                    size="sm"
                    variant="outline"
                    disabled={transition.isPending}
                    onClick={() => void transitionTo(state)}
                  >
                    Move to {lifecycleLabels[state]}
                  </Button>
                ))}
              <span className="text-xs text-muted-foreground">
                Current state: {lifecycleLabels[task.lifecycle.state]}
              </span>
            </div>
          </div>
        </CollapsibleSection>
        {dispatch?.blockers.length ? (
          <section
            className="rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100"
            aria-labelledby="dispatch-blockers-title"
          >
            <h2 id="dispatch-blockers-title" className="font-semibold">
              Work cannot start yet
            </h2>
            <ul className="mt-1 list-disc pl-5">
              {dispatch.blockers.map((blocker) => (
                <li key={`${blocker.code}:${blocker.work_unit_id ?? ''}`}>
                  {launchBlockerMessage(blocker.code, blocker.message)}
                  <details className="mt-1 text-xs">
                    <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                      Technical details
                    </summary>
                    <p className="mt-1 break-all">{blocker.message}</p>
                    {blocker.work_unit_id ? (
                      <code className="break-all">WorkUnit {blocker.work_unit_id}</code>
                    ) : null}
                  </details>
                </li>
              ))}
            </ul>
          </section>
        ) : null}
        <nav
          data-task-navigation="true"
          className="flex min-w-0 gap-2 overflow-x-auto border-b"
          aria-label="Task sections"
        >
          {tabs.map((tab) => (
            <button
              key={tab}
              type="button"
              data-active-task-section={activeTab === tab ? 'true' : undefined}
              aria-pressed={activeTab === tab}
              className={`shrink-0 whitespace-nowrap border-b-2 px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${activeTab === tab ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground'}`}
              onClick={() => setActiveTab(tab)}
            >
              {tabLabels[tab]}
            </button>
          ))}
        </nav>
      </header>

      <Dialog
        open={dispatchConfirmAction !== null}
        onOpenChange={(open) => {
          if (!open && !createIntervention.isPending) setDispatchConfirmAction(null)
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {dispatchConfirmAction === 'pause' ? 'Pause new Task work?' : 'Cancel this Task?'}
            </DialogTitle>
            <DialogDescription>
              {dispatchConfirmAction === 'pause'
                ? 'Forge will stop admitting new work and provider starts. Work already running may continue. Resume rechecks the current Task and assignments before more work starts.'
                : 'The Task will move to Cancelled and stop accepting new work. Forge will request cancellation of running Executions. The Task cannot be resumed; its work and evidence remain in history.'}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={createIntervention.isPending}
              onClick={() => setDispatchConfirmAction(null)}
            >
              Keep Task running
            </Button>
            <Button
              type="button"
              variant={dispatchConfirmAction === 'cancel' ? 'destructive' : 'default'}
              disabled={createIntervention.isPending}
              onClick={() => {
                if (!dispatchConfirmAction) return
                void dispatchAction(dispatchConfirmAction).then((succeeded) => {
                  if (succeeded) setDispatchConfirmAction(null)
                })
              }}
            >
              {createIntervention.isPending
                ? 'Saving…'
                : dispatchConfirmAction === 'pause'
                  ? 'Pause new work'
                  : 'Cancel Task'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={editTaskOpen} onOpenChange={setEditTaskOpen}>
        <DialogContent className="sm:max-w-2xl">
          <form onSubmit={(event) => void saveTask(event)} className="space-y-4">
            <DialogHeader>
              <DialogTitle>Edit task</DialogTitle>
              <DialogDescription>
                Keep the title concise and put the full brief in the Markdown description.
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-2">
              <Label htmlFor="edit-task-title">Title</Label>
              <Input
                id="edit-task-title"
                value={editTitle}
                onChange={(event) => setEditTitle(event.target.value)}
                required
              />
            </div>
            {!task.description?.trim() && editTitle.length > 255 ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => {
                  const fullBrief = editTitle
                  const heading = fullBrief.match(/^#\s+(.+)$/m)?.[1]?.trim()
                  setEditDescription(fullBrief)
                  if (heading) setEditTitle(heading)
                }}
              >
                Move current title into Markdown description
              </Button>
            ) : null}
            <div className="space-y-2">
              <Label>Description (Markdown)</Label>
              <MarkdownEditor
                value={editDescription}
                onChange={setEditDescription}
                minHeight="240px"
                placeholder="Add the task brief and acceptance criteria…"
              />
            </div>
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={() => setEditTaskOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={updateTask.isPending || !editTitle.trim()}>
                {updateTask.isPending ? 'Saving…' : 'Save task'}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {activeTab === 'overview' ? (
        <div className="grid min-w-0 gap-4 lg:grid-cols-2">
          <div className="min-w-0 lg:col-span-2">
            {coordinationQuery.data ? (
              <TaskJourneyPanel
                task={task}
                coordination={coordinationQuery.data}
                dispatch={dispatch}
                workUnits={workUnits}
                reviewLines={reviewLinesQuery.data ?? []}
                gates={gatesQuery.data ?? []}
                agents={agents}
                members={members}
                daemons={daemons}
                onOpenAdvancedWork={() => setActiveTab('work')}
                onOpenReview={() => setActiveTab('review')}
                onOpenResults={() => setActiveTab('artifacts')}
              />
            ) : coordinationQuery.isError ? (
              <p className="rounded-lg border border-destructive/40 p-4 text-sm text-destructive">
                The guided Task flow could not be loaded. Manual Task controls remain available.
              </p>
            ) : (
              <p className="rounded-lg border p-4 text-sm text-muted-foreground">
                Loading the guided Task flow…
              </p>
            )}
          </div>
          <div className="min-w-0 lg:col-span-2">
            <TaskCoordinationPanel
              task={task}
              agents={agents}
              members={members}
              daemons={daemons}
              onOpenAdvancedWork={() => setActiveTab('work')}
            />
          </div>
          <section className="min-w-0 space-y-3 rounded-lg border p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h2 className="font-semibold">Task status</h2>
              <Badge variant="outline">{lifecycleLabels[task.lifecycle.state]}</Badge>
            </div>
            <p className="text-sm text-muted-foreground">
              This is the Task’s durable overall state. Dispatch readiness and review results are
              shown separately.
            </p>
            {task.lifecycle.reason_kind ? (
              <details className="text-xs text-muted-foreground">
                <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                  Lifecycle details
                </summary>
                <div className="mt-2 space-y-1 break-all">
                  <p>Reason: {task.lifecycle.reason_kind}</p>
                  {task.lifecycle.reason_ref ? <p>Reference: {task.lifecycle.reason_ref}</p> : null}
                  <p>
                    Task version {task.version} · lifecycle version {task.lifecycle.version}
                  </p>
                </div>
              </details>
            ) : null}
          </section>
          <section className="min-w-0 space-y-3 rounded-lg border p-4">
            <div className="flex flex-wrap items-start justify-between gap-2">
              <div className="min-w-0">
                <h2 className="font-semibold">Team</h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  Roles, eligible Actors, and the coordinator saved on this Task.
                </p>
              </div>
              <Link
                to="/projects/$projectId/settings/$tab"
                params={{ projectId: task.project_id, tab: 'workers' }}
                className="inline-flex h-8 items-center justify-center rounded-md border border-input bg-card px-[9px] text-xs font-medium hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                Team &amp; templates
              </Link>
            </div>
            {task.task_roles.length ? (
              task.task_roles.map((role) => (
                <article key={role.id} className="min-w-0 rounded-md border p-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 className="break-words text-sm font-medium">
                      {role.configuration.display_label || role.role.replaceAll('_', ' ')}
                    </h3>
                    {role.configuration.capabilities.includes('orchestration') ? (
                      <Badge variant="outline">Coordinator</Badge>
                    ) : null}
                  </div>
                  <div className="mt-2 space-y-2">
                    {role.members
                      .filter((member) => member.status === 'active')
                      .map((member) => (
                        <ActorIdentity
                          key={member.id}
                          actor={member.actor_ref}
                          agents={agents}
                          members={members}
                          daemons={daemons}
                        />
                      ))}
                    {role.members
                      .filter((member) => member.status !== 'active')
                      .map((member) => (
                        <div key={member.id} className="rounded border border-dashed p-2">
                          <ActorIdentity
                            actor={member.actor_ref}
                            agents={agents}
                            members={members}
                            daemons={daemons}
                          />
                          <p className="ml-6 mt-1 text-xs text-muted-foreground">
                            {member.status} membership
                          </p>
                        </div>
                      ))}
                    {!role.members.length ? (
                      <p className="text-xs text-muted-foreground">
                        No members are saved for this role.
                      </p>
                    ) : null}
                  </div>
                  <details className="mt-2 text-xs text-muted-foreground">
                    <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                      Role reference
                    </summary>
                    <code className="mt-1 block break-all">{role.role}</code>
                  </details>
                </article>
              ))
            ) : (
              <p className="text-sm text-muted-foreground">No roles are recorded on this Task.</p>
            )}
            {membersQuery.isError || agentsQuery.isError || daemonsQuery.isError ? (
              <p role="status" className="text-xs text-muted-foreground">
                Some team identity details could not be loaded; references remain available per
                member.
              </p>
            ) : null}
          </section>
          <section className="min-w-0 space-y-3 rounded-lg border p-4 lg:col-span-2">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h2 className="font-semibold">Work and next step</h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  Statuses below come from the live Task dispatch preflight.
                </p>
              </div>
              <Button size="sm" variant="outline" onClick={() => setActiveTab('work')}>
                Open advanced work setup
              </Button>
            </div>
            {!workUnits.length ? (
              <div className="rounded-md border border-dashed p-4">
                <h3 className="text-sm font-medium">No work items prepared</h3>
                <p className="mt-1 text-sm text-muted-foreground">
                  This is a valid empty Task. Describe the objective in Plan and next step to prepare
                  a proposal, or use Advanced work for manual roles and WorkUnits.
                </p>
              </div>
            ) : (
              <ul className="grid min-w-0 gap-2 md:grid-cols-2">
                {(dispatch?.work_units ?? []).map((unit) => {
                  const role = task.task_roles.find((item) => item.role === unit.role)
                  const statusLabels: Record<string, string> = {
                    ready: 'Ready to start',
                    running: 'In progress',
                    waiting_human: 'Waiting for a person',
                    waiting_dependency: 'Waiting for prerequisite work',
                    awaiting_acceptance: 'Result needs acceptance',
                    awaiting_review_approval: 'Waiting for review approval',
                    awaiting_integration: 'Waiting for integration',
                    awaiting_recovery: 'Needs recovery',
                    unassigned: 'Needs an assignment',
                    blocked: 'Blocked',
                    completed: 'Complete',
                    cancelled: 'Cancelled',
                  }
                  return (
                    <li key={unit.work_unit_id} className="min-w-0 rounded-md border p-3">
                      <div className="flex flex-wrap items-start justify-between gap-2">
                        <div className="min-w-0">
                          <h3 className="break-words text-sm font-medium">{unit.title}</h3>
                          <p className="mt-0.5 text-xs text-muted-foreground">
                            {role?.configuration.display_label || unit.role.replaceAll('_', ' ')}
                          </p>
                        </div>
                        <Badge variant="outline">
                          {statusLabels[unit.state] || 'Status unavailable'}
                        </Badge>
                      </div>
                      {unit.assigned_actor ? (
                        <ActorIdentity
                          className="mt-2"
                          actor={unit.assigned_actor}
                          agents={agents}
                          members={members}
                          daemons={daemons}
                        />
                      ) : (
                        <p className="mt-2 text-xs text-muted-foreground">No Actor assigned</p>
                      )}
                      {unit.reason ? (
                        <p className="mt-2 text-xs text-muted-foreground">{unit.reason}</p>
                      ) : null}
                      <details className="mt-2 text-xs text-muted-foreground">
                        <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                          Work item reference
                        </summary>
                        <div className="mt-1 flex min-w-0 items-center gap-1">
                          <code className="min-w-0 break-all">{unit.work_unit_id}</code>
                          <CopyReferenceButton value={unit.work_unit_id} label="WorkUnit ID" />
                        </div>
                      </details>
                    </li>
                  )
                })}
              </ul>
            )}
            <CollapsibleSection
              title="Review and integration checks"
              badge={gatesQuery.data?.length}
            >
              {gatesQuery.isLoading ? (
                <p className="text-sm text-muted-foreground">Loading checks…</p>
              ) : null}
              {gatesQuery.isError ? (
                <p role="alert" className="text-sm text-destructive">
                  Review and integration checks could not be loaded.
                </p>
              ) : null}
              {gatesQuery.data?.map(({ gate, active_policy: policy }) => {
                const evaluation = evaluations[gate.id]
                const evaluationIssues = Array.isArray(evaluation?.result.issues)
                  ? evaluation.result.issues.filter(
                      (issue): issue is string => typeof issue === 'string',
                    )
                  : []
                return (
                  <article key={gate.id} className="mt-3 space-y-3 rounded-md border p-3">
                    <div className="flex flex-wrap items-center gap-2">
                      <strong>{gate.gate_kind.replaceAll('_', ' ')}</strong>
                      <Badge variant="outline">
                        policy revision {policy ? String(policy.revision) : 'unavailable'}
                      </Badge>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={evaluateGate.isPending}
                        onClick={() => void evaluate(gate.id)}
                      >
                        Evaluate current inputs
                      </Button>
                      <details className="text-xs text-muted-foreground">
                        <summary className="cursor-pointer">Technical reference</summary>
                        <code className="break-all">Gate {gate.id}</code>
                      </details>
                    </div>
                    {evaluation ? (
                      <div className="space-y-2">
                        <div className="flex flex-wrap items-center gap-2 text-xs">
                          <Badge
                            variant={
                              evaluation.outcome === 'satisfied' ? 'outline' : 'destructive'
                            }
                          >
                            {evaluation.outcome === 'satisfied'
                              ? 'Exact inputs satisfy this check'
                              : 'Exact inputs do not satisfy this check'}
                          </Badge>
                          {!evaluationIssues.length ? null : (
                            <span className="text-muted-foreground">
                              {evaluationIssues.length} recorded issue
                              {evaluationIssues.length === 1 ? '' : 's'}
                            </span>
                          )}
                        </div>
                        <details className="text-xs text-muted-foreground">
                          <summary className="w-fit cursor-pointer">Evaluation reference</summary>
                          <p className="mt-1 break-all">
                            GateEvaluation {evaluation.id} · input digest {evaluation.input_digest}
                          </p>
                          {evaluationIssues.length ? (
                            <ul className="mt-1 list-disc space-y-1 pl-5">
                              {evaluationIssues.map((issue) => (
                                <li key={issue} className="break-words">
                                  {issue.replaceAll('_', ' ')}
                                </li>
                              ))}
                            </ul>
                          ) : null}
                        </details>
                      </div>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        Evaluate the current immutable inputs before using this check for delivery.
                      </p>
                    )}
                    {evaluation?.outcome === 'satisfied' &&
                    ['active', 'ready', 'blocked'].includes(task.lifecycle.state) ? (
                      <Button
                        size="sm"
                        disabled={transition.isPending}
                        onClick={() => void transitionTo('ready_to_merge', evaluation.id)}
                      >
                        Mark ready for integration
                      </Button>
                    ) : null}
                    {task.lifecycle.state === 'ready_to_merge' &&
                    evaluation?.outcome === 'satisfied' ? (
                      <Button
                        size="sm"
                        disabled={transition.isPending}
                        onClick={() => void merge(task, evaluation)}
                      >
                        Admit integration
                      </Button>
                    ) : null}
                  </article>
                )
              })}
              {!gatesQuery.isLoading && !gatesQuery.isError && !gatesQuery.data?.length ? (
                <p className="mt-3 text-sm text-muted-foreground">
                  No active review or integration check is recorded.
                </p>
              ) : null}
              {!hasMergeReadinessGate &&
              reviewLinesQuery.data?.length &&
              task.lifecycle.state !== 'done' &&
              task.lifecycle.state !== 'cancelled' ? (
                <div className="mt-3 space-y-2 rounded-md border border-dashed p-3">
                  <p className="text-sm">
                    No <strong>merge readiness</strong> check exists for this Task. A correction can
                    advance the review line, so the old work-item Gate may no longer cover the
                    current result.
                  </p>
                  <p className="text-xs text-muted-foreground">
                    This creates a Task-scoped Gate over review line revision{' '}
                    {String(mergeGateReviewLine?.revision ?? 0)}
                    {mergeGateReviewLine?.latest_verdict
                      ? ` (${mergeGateReviewLine.latest_verdict.replaceAll('_', ' ')})`
                      : ' (no verdict yet)'}
                    . It evaluates exact review inputs; it does not run tests, review code,
                    integrate, or merge.
                  </p>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={creatingMergeGate}
                    onClick={() => void createMergeReadinessGate()}
                  >
                    {creatingMergeGate
                      ? 'Creating check…'
                      : 'Create merge-readiness check'}
                  </Button>
                </div>
              ) : null}
            </CollapsibleSection>
          </section>
          <section className="min-w-0 space-y-3 rounded-lg border p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h2 className="font-semibold">Review</h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  Verdicts attach to exact results and revisions.
                </p>
              </div>
              <Button size="sm" variant="outline" onClick={() => setActiveTab('review')}>
                View review
              </Button>
            </div>
            {reviewLinesQuery.isLoading ? (
              <p className="text-sm text-muted-foreground">Loading review status…</p>
            ) : null}
            {reviewLinesQuery.isError ? (
              <p role="alert" className="text-sm text-destructive">
                Review status could not be loaded.
              </p>
            ) : null}
            {(reviewLinesQuery.data ?? []).map((line) => (
              <div
                key={line.id}
                className="flex min-w-0 flex-wrap items-center gap-2 rounded-md border p-3 text-sm"
              >
                <Badge variant="outline">
                  {line.latest_verdict === 'pass'
                    ? 'Passed'
                    : line.latest_verdict === 'request_changes'
                      ? 'Changes requested'
                      : line.latest_verdict?.replaceAll('_', ' ') || 'Awaiting review'}
                </Badge>
                <span>Revision {line.revision}</span>
                <details className="ml-auto text-xs text-muted-foreground">
                  <summary className="cursor-pointer">Review references</summary>
                  <p className="mt-1 break-all">
                    Review line {line.id} · result Artifact{' '}
                    {line.current_result_artifact_id ?? 'not recorded'} · ReviewReport{' '}
                    {line.latest_report_artifact_id ?? 'not recorded'}
                  </p>
                </details>
              </div>
            ))}
            {!reviewLinesQuery.isLoading &&
            !reviewLinesQuery.isError &&
            !reviewLinesQuery.data?.length ? (
              <p className="text-sm text-muted-foreground">No review result is recorded yet.</p>
            ) : null}
          </section>
          <section className="min-w-0 space-y-3 rounded-lg border p-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h2 className="font-semibold">Results</h2>
                <p className="mt-1 text-xs text-muted-foreground">
                  Work completion and review approval are separate facts.
                </p>
              </div>
              <Button size="sm" variant="outline" onClick={() => setActiveTab('artifacts')}>
                View results
              </Button>
            </div>
            {workUnits.filter((unit) => unit.status === 'completed').length ? (
              <p className="text-sm">
                {workUnits.filter((unit) => unit.status === 'completed').length} of{' '}
                {workUnits.length} work items are marked complete.
              </p>
            ) : (
              <p className="text-sm text-muted-foreground">
                No completed work item is recorded yet.
              </p>
            )}
            <p className="text-xs text-muted-foreground">
              Inspect Artifacts and manifests in Results. A completed work item alone does not mean
              its review passed or the Task is done.
            </p>
          </section>
        </div>
      ) : null}

      {activeTab === 'work' ? (
        <TaskWorkManagement
          task={task}
          workUnits={workUnits}
          executions={executions}
          onChanged={async () => {
            await Promise.all([
              taskQuery.refetch(),
              workUnitsQuery.refetch(),
              dispatchQuery.refetch(),
            ])
          }}
        />
      ) : null}

      {activeTab === 'executions' ? (
        <section className="space-y-3 rounded-lg border p-4">
          <h2 className="font-semibold">Executions</h2>
          <p className="text-xs text-muted-foreground">
            Execution records are technical history for dispatched Agent WorkUnits and submitted
            Human review WorkUnits. This view is historical Activity; it is not a launch form. Pause
            and cancel act on the Task dispatch intent.
          </p>
          {dispatch ? (
            <p className="text-sm">
              {dispatch.dispatch.state === 'paused'
                ? 'Dispatch is paused.'
                : dispatch.dispatch.state === 'running'
                  ? 'Dispatch is accepting work.'
                  : dispatch.dispatch.state === 'cancelled'
                    ? 'Dispatch is cancelled.'
                    : 'Dispatch has not started.'}{' '}
              {dispatch.work_units.filter((unit) => unit.state === 'ready').length} ready WorkUnits
            </p>
          ) : null}
          {executions.map((execution) => (
            <article key={execution.id} className="space-y-1 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{execution.status}</Badge>
                <span>
                  {task.task_roles.find((role) => role.role === execution.role)?.configuration
                    .display_label ?? execution.role.replaceAll('_', ' ')}
                </span>
                <details className="ml-auto text-xs text-muted-foreground">
                  <summary className="cursor-pointer">Technical references</summary>
                  <div className="mt-1 space-y-1 break-all">
                    <p>Execution {execution.id}</p>
                    <p>
                      Actor {execution.actor_ref?.kind ?? 'unavailable'}:
                      {execution.actor_ref?.id ?? 'unavailable'}
                    </p>
                    <p>
                      Harness session {execution.harness_session_id ?? 'none'} · Workspace{' '}
                      {execution.workspace_id ?? 'none'}
                    </p>
                  </div>
                </details>
              </div>
            </article>
          ))}
          {executions.length === 0 ? (
            <p className="text-sm text-muted-foreground">No Execution records are available.</p>
          ) : null}
        </section>
      ) : null}

      {activeTab === 'artifacts' ? <TaskArtifactsPanel taskId={task.id} /> : null}

      {activeTab === 'review' ? (
        <section className="space-y-4">
          <div className="rounded-lg border p-4">
            <h2 className="font-semibold">Review Executions</h2>
            <p className="text-xs text-muted-foreground">
              Formal Review is prepared as a WorkUnit for a TaskRole with the formal_review
              capability and an exact subject. Assigned Humans submit their report from that
              WorkUnit; the Task dispatcher records Agent attempts.
            </p>
          </div>
          <section className="space-y-3 rounded-lg border p-4">
            <h3 className="font-semibold">Review lines and approval state</h3>
            <p className="text-xs text-muted-foreground">
              A correction advances its line and clears the previous verdict. Consumers that require
              approval can run only after a Gate evaluation covers the current passing revision.
            </p>
            {reviewLinesQuery.isLoading ? (
              <p className="text-sm text-muted-foreground">Loading review lines…</p>
            ) : null}
            {(reviewLinesQuery.data ?? []).map((line) => (
              <article key={line.id} className="space-y-2 rounded-md border p-3 text-xs">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant="outline">
                    {line.latest_verdict === 'pass'
                      ? 'Passed'
                      : line.latest_verdict === 'request_changes'
                        ? 'Changes requested'
                        : line.latest_verdict?.replaceAll('_', ' ') || 'Awaiting review'}
                  </Badge>
                  <span>Revision {String(line.revision)}</span>
                </div>
                <details>
                  <summary className="w-fit cursor-pointer rounded-sm text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                    Exact result and review history
                  </summary>
                  <div className="mt-2 space-y-2 break-all">
                    <p>
                      Review line {line.id} · current WorkUnit {line.current_work_unit_id} v
                      {String(line.current_work_unit_version)}
                    </p>
                    <p>
                      Result Execution {line.current_result_execution_id ?? 'pending'} · Artifact{' '}
                      {line.current_result_artifact_id ?? 'none'} · manifest{' '}
                      {line.current_manifest_id ?? 'none'}
                    </p>
                    <div className="space-y-1 border-l pl-3">
                      {line.history.map((revision) => (
                        <p key={`${line.id}:${revision.revision}`}>
                          Revision {String(revision.revision)} · WorkUnit {revision.work_unit_id} v
                          {String(revision.work_unit_version)} ·{' '}
                          {revision.verdict ?? 'awaiting result or review'}
                          {revision.review_report_artifact_id
                            ? ` · report ${revision.review_report_artifact_id}`
                            : ''}
                          {revision.manifest_id ? ` · manifest ${revision.manifest_id}` : ''}
                        </p>
                      ))}
                    </div>
                  </div>
                </details>
              </article>
            ))}
            {!reviewLinesQuery.isLoading && (reviewLinesQuery.data?.length ?? 0) === 0 ? (
              <p className="text-sm text-muted-foreground">
                No durable review line is recorded yet.
              </p>
            ) : null}
            {reviewLinesQuery.isError ? (
              <p role="alert" className="text-sm text-destructive">
                Review lines could not be loaded.
              </p>
            ) : null}
          </section>
          {workUnits.some(
            (unit) =>
              task.task_roles
                .find((role) => role.role === unit.role)
                ?.configuration.capabilities.includes('formal_review') &&
              unit.status === 'open' &&
              unit.assigned_actor?.kind === 'human' &&
              unit.assigned_actor.id === userId,
          ) ? (
            <section className="space-y-3">
              <h3 className="font-semibold">Human review WorkUnits assigned to you</h3>
              {workUnits
                .filter(
                  (unit) =>
                    task.task_roles
                      .find((role) => role.role === unit.role)
                      ?.configuration.capabilities.includes('formal_review') &&
                    unit.status === 'open' &&
                    unit.assigned_actor?.kind === 'human' &&
                    unit.assigned_actor.id === userId,
                )
                .map((unit) => (
                  <WorkUnitHumanReviewPanel
                    key={unit.id}
                    task={task}
                    unit={unit}
                    currentUserId={userId}
                    workUnits={workUnits}
                  />
                ))}
            </section>
          ) : null}
          {reviewsQuery.isLoading ? (
            <p className="text-sm text-muted-foreground">Loading Review Executions…</p>
          ) : null}
          {reviews.map(({ execution, report }) => (
            <div key={execution.id} className="space-y-2">
              <ReviewExecutionCard
                review={{ execution, report }}
                roleLabel={
                  task.task_roles.find((role) => role.role === execution.role)?.configuration
                    .display_label ?? undefined
                }
              />
              <TaskReworkPanel
                task={task}
                review={{ execution, report }}
                agents={agents}
                members={members}
                daemons={daemons}
                workUnits={workUnits}
                onChanged={async () => {
                  await Promise.all([
                    taskQuery.refetch(),
                    workUnitsQuery.refetch(),
                    reviewLinesQuery.refetch(),
                    dispatchQuery.refetch(),
                    executionsQuery.refetch(),
                  ])
                }}
              />
            </div>
          ))}
          {!reviewsQuery.isLoading && reviews.length === 0 ? (
            <p className="rounded-lg border p-4 text-sm text-muted-foreground">
              No Review Execution is recorded.
            </p>
          ) : null}
          <section className="space-y-2 rounded-lg border p-4">
            <h3 className="font-semibold">ValidationRuns and Evidence</h3>
            {validations.map((run) => (
              <article key={run.id} className="space-y-1 rounded border p-3 text-xs">
                <div className="flex flex-wrap gap-2">
                  <Badge variant="outline">{run.status}</Badge>
                  <strong>{run.check_identity}</strong>
                </div>
                <details>
                  <summary className="w-fit cursor-pointer text-muted-foreground">
                    Technical evidence
                  </summary>
                  <div className="mt-1 space-y-1 break-all">
                    <p>
                      ValidationRun {run.id} · Workspace {run.workspace_id} · commit{' '}
                      {run.commit_sha}
                    </p>
                    <p>Snapshot {run.workspace_snapshot_digest}</p>
                    <p>
                      Evidence references:{' '}
                      {run.evidence_ids.length ? run.evidence_ids.join(', ') : 'none'}
                    </p>
                  </div>
                </details>
              </article>
            ))}
            {validations.length === 0 ? (
              <p className="text-sm text-muted-foreground">No ValidationRun is recorded.</p>
            ) : null}
          </section>
        </section>
      ) : null}

      {activeTab === 'terminal' ? <TaskTerminalPanel taskId={taskId} /> : null}

      {activeTab === 'history' ? (
        <section className="space-y-3 rounded-lg border p-4">
          <h2 className="font-semibold">TaskLifecycle transition facts</h2>
          {(transitionsQuery.data ?? []).map((fact) => (
            <article key={fact.id} className="space-y-1 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap gap-2">
                <Badge variant="outline">
                  {fact.from_state} → {fact.to_state}
                </Badge>
                <span>
                  version {String(fact.from_version)} → {String(fact.to_version)}
                </span>
              </div>
              <details className="text-xs text-muted-foreground">
                <summary className="w-fit cursor-pointer">Technical references</summary>
                <p className="mt-1 break-all">Transition {fact.id}</p>
              </details>
              <p className="text-xs text-muted-foreground">
                Cause {fact.cause_kind}
                {fact.cause_ref ? ' · reference recorded' : ''}
                {fact.gate_evaluation_id ? ' · GateEvaluation reference recorded' : ''}
              </p>
              <p className="text-xs text-muted-foreground">{fact.created_at}</p>
            </article>
          ))}
          {(transitionsQuery.data?.length ?? 0) === 0 ? (
            <p className="text-sm text-muted-foreground">No lifecycle transitions are recorded.</p>
          ) : null}
        </section>
      ) : null}
    </main>
  )
}

export function ReviewExecutionCard({
  review,
  roleLabel,
}: {
  review: ReviewExecutionResponse
  roleLabel?: string
}) {
  const { execution, report } = review

  return (
    <article className="space-y-3 rounded-lg border p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{execution.status}</Badge>
        <Badge variant="outline">
          {roleLabel ?? execution.role.replaceAll('_', ' ')}
        </Badge>
      </div>
      {report ? (
        <div className="space-y-2 rounded bg-muted/40 p-3">
          <div className="flex flex-wrap gap-2">
            <Badge>ReviewReport</Badge>
          </div>
          <p className="text-sm">{report.content ?? 'Report content is not available.'}</p>
        </div>
      ) : null}
      <details className="text-xs text-muted-foreground">
        <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          Technical references
        </summary>
        <div className="mt-1 space-y-1 break-all">
          <p>Execution {execution.id}</p>
          <p>
            Actor {execution.actor_ref?.kind ?? 'unknown'}:{execution.actor_ref?.id ?? 'unknown'} ·
            Workspace {execution.workspace_id ?? 'none'}
          </p>
          <p>
            Base {execution.before_sha ?? 'none'} · head {execution.after_sha ?? 'none'}
          </p>
          {report ? <p>ReviewReport Artifact {report.id}</p> : null}
        </div>
      </details>
    </article>
  )
}
