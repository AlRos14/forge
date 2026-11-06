import { useState, type FormEvent } from 'react'
import { Link } from '@tanstack/react-router'
import { toast } from 'sonner'
import {
  useCancelTask,
  useEvaluateGate,
  useExecutionsQuery,
  useLaunchTask,
  usePauseTask,
  useResumeTask,
  useReviewsQuery,
  useTaskDispatchQuery,
  useTaskGatesQuery,
  useTaskLifecycleTransitionsQuery,
  useTaskQuery,
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
import type {
  GateEvaluationResponse,
  MergeAfterGateResponse,
  ReviewExecutionResponse,
  Task,
  TaskDispatchActionRequest,
  TaskLifecycleState,
} from '@/types/generated'

export type TaskDetailTab = 'overview' | 'work' | 'executions' | 'artifacts' | 'review' | 'terminal' | 'history'

export function isTaskDetailTab(value: string | undefined): value is TaskDetailTab {
  return value === 'overview' || value === 'work' || value === 'executions' || value === 'artifacts' || value === 'review' || value === 'terminal' || value === 'history'
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

const tabs: TaskDetailTab[] = ['overview', 'work', 'executions', 'artifacts', 'review', 'terminal', 'history']

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
  const validationsQuery = useValidationsQuery(taskId)
  const gatesQuery = useTaskGatesQuery(taskId)
  const workUnitsQuery = useTaskWorkUnitsQuery(taskId)
  const dispatchQuery = useTaskDispatchQuery(taskId)
  const transition = useTransitionTaskLifecycle()
  const updateTask = useUpdateTask()
  const evaluateGate = useEvaluateGate()
  const launchTask = useLaunchTask()
  const pauseTask = usePauseTask()
  const resumeTask = useResumeTask()
  const cancelTask = useCancelTask()
  const userId = useAuthStore((state) => state.user?.id)
  const [evaluations, setEvaluations] = useState<Record<string, GateEvaluationResponse>>({})
  const [editTaskOpen, setEditTaskOpen] = useState(false)
  const [editTitle, setEditTitle] = useState('')
  const [editDescription, setEditDescription] = useState('')

  const task = taskQuery.data
  const reviews = reviewsQuery.data ?? []
  const validations = validationsQuery.data ?? []
  const executions = executionsQuery.data?.items ?? []
  const workUnits = workUnitsQuery.data ?? []
  const dispatch = dispatchQuery.data

  async function transitionTo(toState: TaskLifecycleState) {
    if (!task) return
    try {
      await transition.mutateAsync({
        taskId: task.id,
        toState,
        expectedLifecycleVersion: task.lifecycle.version,
        idempotencyKey: crypto.randomUUID(),
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
    if (!task || !dispatch) return
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
      } else if (action === 'pause') {
        await pauseTask.mutateAsync({ taskId: task.id, body })
        toast.success('Task pause admitted; running Executions were not claimed as stopped.')
      } else if (action === 'resume') {
        await resumeTask.mutateAsync({ taskId: task.id, body })
        toast.success('Task dispatch resumed.')
      } else {
        await cancelTask.mutateAsync({ taskId: task.id, body })
        toast.success('Task cancellation requested.')
      }
      await Promise.all([taskQuery.refetch(), executionsQuery.refetch(), workUnitsQuery.refetch(), dispatchQuery.refetch()])
    } catch (error) {
      toast.error(getApiErrorMessage(error, `Task ${action} failed`))
    }
  }

  if (taskQuery.isLoading) {
    return <div className="p-6 text-sm text-muted-foreground">Loading task…</div>
  }
  if (taskQuery.isError) {
    return <div className="p-6"><ErrorBanner error={taskQuery.error} fallback="Task failed to load" /></div>
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
            <h1 className="text-2xl font-semibold">{task.title}</h1>
            {task.description ? (
              <MarkdownView content={task.description} className="mt-2 text-sm text-muted-foreground" />
            ) : null}
          </div>
          <Button size="sm" variant="outline" onClick={openTaskEditor}>
            Edit task
          </Button>
          <Badge variant="outline">{task.lifecycle.state.replaceAll('_', ' ')}</Badge>
        </div>
        <div className="flex flex-wrap gap-2">
          {(nextLifecycleStates[task.lifecycle.state] ?? []).map((state) => (
            <Button
              key={state}
              size="sm"
              variant={state === 'cancelled' ? 'outline' : 'default'}
              disabled={transition.isPending}
              onClick={() => void transitionTo(state)}
            >
              Move to {state.replaceAll('_', ' ')}
            </Button>
          ))}
          <span className="mx-1 h-8 w-px bg-border" aria-hidden="true" />
          {dispatch?.can_launch ? (
            <Button size="sm" disabled={launchTask.isPending} onClick={() => void dispatchAction('launch')}>
              Launch Task
            </Button>
          ) : null}
          {dispatch?.can_pause ? (
            <Button size="sm" variant="outline" disabled={pauseTask.isPending} onClick={() => void dispatchAction('pause')}>
              Pause dispatch
            </Button>
          ) : null}
          {dispatch?.can_resume ? (
            <Button size="sm" variant="outline" disabled={resumeTask.isPending} onClick={() => void dispatchAction('resume')}>
              Resume dispatch
            </Button>
          ) : null}
          {dispatch?.can_cancel ? (
            <Button size="sm" variant="outline" disabled={cancelTask.isPending} onClick={() => void dispatchAction('cancel')}>
              Cancel Task
            </Button>
          ) : null}
        </div>
        {dispatch?.blockers.length ? (
          <div className="rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-950">
            <strong>Launch blockers</strong>
            <ul className="mt-1 list-disc pl-5">
              {dispatch.blockers.map((blocker) => (
                <li key={`${blocker.code}:${blocker.work_unit_id ?? ''}`}>
                  {blocker.message}
                  {blocker.work_unit_id ? ` (${blocker.work_unit_id})` : ''}
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        <nav className="flex gap-2 border-b" aria-label="Task sections">
          {tabs.map((tab) => (
            <button
              key={tab}
              type="button"
              className={`border-b-2 px-3 py-2 text-sm capitalize ${activeTab === tab ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground'}`}
              onClick={() => setActiveTab(tab)}
            >
              {tab}
            </button>
          ))}
        </nav>
      </header>

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
        <div className="grid gap-4 lg:grid-cols-2">
          <section className="space-y-3 rounded-lg border p-4">
            <h2 className="font-semibold">TaskLifecycle</h2>
            <p className="text-sm">State: <strong>{task.lifecycle.state}</strong></p>
            <p className="text-sm">Task version: {task.version} · lifecycle version: {String(task.lifecycle.version)}</p>
            <p className="text-sm">Reason: {task.lifecycle.reason_kind ?? 'none'}{task.lifecycle.reason_ref ? ` · ${task.lifecycle.reason_ref}` : ''}</p>
            <p className="text-xs text-muted-foreground">Lifecycle is aggregate progress. Actor cognition and execution history are recorded separately.</p>
          </section>
          <section className="space-y-3 rounded-lg border p-4">
            <h2 className="font-semibold">Work assignment</h2>
            <p className="text-sm">{task.task_roles.length} TaskRoles · {workUnits.length} WorkUnits</p>
            <Button size="sm" variant="outline" onClick={() => setActiveTab('work')}>Manage TaskRoles and WorkUnits</Button>
          </section>
          <section className="space-y-3 rounded-lg border p-4 lg:col-span-2">
            <div className="flex items-center justify-between gap-3">
              <h2 className="font-semibold">Gates</h2>
              <span className="text-xs text-muted-foreground">Each evaluation pins exact policy and input facts.</span>
            </div>
            {gatesQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading Gates…</p> : null}
            {gatesQuery.data?.map(({ gate, active_policy: policy }) => {
              const evaluation = evaluations[gate.id]
              return (
                <article key={gate.id} className="space-y-3 rounded-md border p-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <strong>{gate.gate_kind}</strong>
                    <Badge variant="outline">policy revision {policy ? String(policy.revision) : 'unavailable'}</Badge>
                    <code className="text-xs text-muted-foreground">Gate {gate.id}</code>
                    <Button size="sm" variant="outline" disabled={evaluateGate.isPending} onClick={() => void evaluate(gate.id)}>Evaluate exact current inputs</Button>
                  </div>
                  {policy ? <p className="break-all text-xs text-muted-foreground">Policy digest: {policy.policy_digest}</p> : null}
                  {evaluation ? (
                    <div className="space-y-2 rounded bg-muted/40 p-3">
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge>{evaluation.outcome}</Badge>
                        <code className="text-xs">GateEvaluation {evaluation.id}</code>
                        <span className="text-xs">policy revision {String(evaluation.policy_revision)}</span>
                      </div>
                      <p className="break-all text-xs">Input digest: {evaluation.input_digest}</p>
                      <ul className="space-y-1 text-xs">
                        {evaluation.inputs.map((input) => (
                          <li key={`${input.ordinal}-${input.input_id}`} className="break-all">
                            {input.input_kind} {input.input_id} v{String(input.input_version)} · {input.status} · {input.input_digest}
                          </li>
                        ))}
                      </ul>
                      {task.lifecycle.state === 'ready_to_merge' && evaluation.outcome === 'satisfied' ? (
                        <Button size="sm" disabled={transition.isPending} onClick={() => void merge(task, evaluation)}>Merge using GateEvaluation {evaluation.id}</Button>
                      ) : null}
                    </div>
                  ) : null}
                </article>
              )
            })}
            {!gatesQuery.isLoading && (gatesQuery.data?.length ?? 0) === 0 ? <p className="text-sm text-muted-foreground">No Gate with an active policy is recorded.</p> : null}
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
            Execution records are technical history for dispatched Agent WorkUnits and submitted Human review
            WorkUnits. This view is historical Activity; it is not a launch form. Pause and cancel act on the
            Task dispatch intent.
          </p>
          {dispatch ? (
            <p className="text-sm">
              Dispatch state: <strong>{dispatch.dispatch.state}</strong> · version {String(dispatch.dispatch.version)} · {dispatch.work_units.filter((unit) => unit.state === 'ready').length} ready WorkUnits
            </p>
          ) : null}
          {executions.map((execution) => (
            <article key={execution.id} className="space-y-1 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{execution.status}</Badge>
                <span>role: {execution.role}</span>
                <code className="ml-auto text-xs">{execution.id}</code>
              </div>
              <p className="text-xs text-muted-foreground">Actor: {execution.actor_ref?.kind ?? 'unavailable'}:{execution.actor_ref?.id ?? 'unavailable'} · Harness session: {execution.harness_session_id ?? 'none'} · Workspace: {execution.workspace_id ?? 'none'}</p>
            </article>
          ))}
          {executions.length === 0 ? <p className="text-sm text-muted-foreground">No Execution records are available.</p> : null}
        </section>
      ) : null}

      {activeTab === 'artifacts' ? (
        <TaskArtifactsPanel taskId={task.id} />
      ) : null}

      {activeTab === 'review' ? (
        <section className="space-y-4">
          <div className="rounded-lg border p-4">
            <h2 className="font-semibold">Review Executions</h2>
            <p className="text-xs text-muted-foreground">Formal Review is prepared as a reviewer WorkUnit with an exact subject. Assigned Human reviewers submit their report from that WorkUnit; the Task dispatcher records Agent reviewer attempts.</p>
          </div>
          {workUnits.some(
            (unit) =>
              unit.role === 'reviewer' &&
              unit.status === 'open' &&
              unit.assigned_actor?.kind === 'human' &&
              unit.assigned_actor.id === userId,
          ) ? (
            <section className="space-y-3">
              <h3 className="font-semibold">Human review WorkUnits assigned to you</h3>
              {workUnits
                .filter(
                  (unit) =>
                    unit.role === 'reviewer' &&
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
                  />
                ))}
            </section>
          ) : null}
          {reviewsQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading Review Executions…</p> : null}
          {reviews.map(({ execution, report }) => (
            <div key={execution.id} className="space-y-2">
              <ReviewExecutionCard review={{ execution, report }} />
              <TaskReworkPanel
                task={task}
                review={{ execution, report }}
                onChanged={async () => {
                  await Promise.all([
                    taskQuery.refetch(),
                    workUnitsQuery.refetch(),
                    dispatchQuery.refetch(),
                    executionsQuery.refetch(),
                  ])
                }}
              />
            </div>
          ))}
          {!reviewsQuery.isLoading && reviews.length === 0 ? <p className="rounded-lg border p-4 text-sm text-muted-foreground">No Review Execution is recorded.</p> : null}
          <section className="space-y-2 rounded-lg border p-4">
            <h3 className="font-semibold">ValidationRuns and Evidence</h3>
            {validations.map((run) => (
              <article key={run.id} className="space-y-1 rounded border p-3 text-xs">
                <div className="flex flex-wrap gap-2"><Badge variant="outline">{run.status}</Badge><strong>{run.check_identity}</strong><code>ValidationRun {run.id}</code></div>
                <p>Workspace {run.workspace_id} · commit {run.commit_sha}</p>
                <p className="break-all">Snapshot {run.workspace_snapshot_digest}</p>
                <p>Evidence IDs: {run.evidence_ids.length ? run.evidence_ids.join(', ') : 'none'}</p>
              </article>
            ))}
            {validations.length === 0 ? <p className="text-sm text-muted-foreground">No ValidationRun is recorded.</p> : null}
          </section>
        </section>
      ) : null}

      {activeTab === 'terminal' ? <TaskTerminalPanel taskId={taskId} /> : null}

      {activeTab === 'history' ? (
        <section className="space-y-3 rounded-lg border p-4">
          <h2 className="font-semibold">TaskLifecycle transition facts</h2>
          {(transitionsQuery.data ?? []).map((fact) => (
            <article key={fact.id} className="space-y-1 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap gap-2"><Badge variant="outline">{fact.from_state} → {fact.to_state}</Badge><span>version {String(fact.from_version)} → {String(fact.to_version)}</span><code className="ml-auto text-xs">transition {fact.id}</code></div>
              <p className="text-xs text-muted-foreground">Cause {fact.cause_kind}{fact.cause_ref ? ` · ${fact.cause_ref}` : ''}{fact.gate_evaluation_id ? ` · GateEvaluation ${fact.gate_evaluation_id}` : ''}</p>
              <p className="text-xs text-muted-foreground">DomainEvent {fact.domain_event_id} · {fact.created_at}</p>
            </article>
          ))}
          {(transitionsQuery.data?.length ?? 0) === 0 ? <p className="text-sm text-muted-foreground">No lifecycle transitions are recorded.</p> : null}
        </section>
      ) : null}
    </main>
  )
}


export function ReviewExecutionCard({
  review,
}: {
  review: ReviewExecutionResponse
}) {
  const { execution, report } = review

  return (
    <article className="space-y-3 rounded-lg border p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{execution.status}</Badge>
        <Badge variant="outline">{execution.role}</Badge>
        <code className="ml-auto text-xs">Execution {execution.id}</code>
      </div>
      <p className="text-xs text-muted-foreground">Actor {execution.actor_ref?.kind ?? 'unknown'}:{execution.actor_ref?.id ?? 'unknown'} · Workspace {execution.workspace_id ?? 'none'} · base {execution.before_sha ?? 'none'} · head {execution.after_sha ?? 'none'}</p>
      {report ? (
        <div className="space-y-2 rounded bg-muted/40 p-3">
          <div className="flex flex-wrap gap-2"><Badge>ReviewReport</Badge><code className="text-xs">Artifact {report.id}</code></div>
          <p className="text-sm">{report.content ?? 'Report content is not available.'}</p>
        </div>
      ) : null}
    </article>
  )
}
