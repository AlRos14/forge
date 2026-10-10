import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { toast } from 'sonner'
import {
  useEvaluateGate,
  useExecutionsQuery,
  useReviewsQuery,
  useStartExecution,
  useSubmitReviewReport,
  useTaskGatesQuery,
  useTaskLifecycleTransitionsQuery,
  useTaskQuery,
  useTransitionTaskLifecycle,
  useTriggerReview,
  useValidationsQuery,
} from '@/api/hooks'
import { apiFetch } from '@/api/client'
import { ErrorBanner } from '@/components/error-banner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import { useAuthStore } from '@/stores/auth'
import { TaskTerminalPanel } from '@/components/task-detail/task-terminal-panel'
import type {
  GateEvaluationResponse,
  MergeAfterGateResponse,
  ReviewExecutionResponse,
  ExecutionPurpose,
  StartExecutionRequest,
  SubmitReviewReportRequest,
  Task,
  TaskLifecycleState,
} from '@/types/generated'

export type TaskDetailTab = 'overview' | 'executions' | 'review' | 'terminal' | 'history'

export function isTaskDetailTab(value: string | undefined): value is TaskDetailTab {
  return value === 'overview' || value === 'executions' || value === 'review' || value === 'terminal' || value === 'history'
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

const tabs: TaskDetailTab[] = ['overview', 'executions', 'review', 'terminal', 'history']

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
  const transition = useTransitionTaskLifecycle()
  const evaluateGate = useEvaluateGate()
  const startReview = useTriggerReview()
  const startExecution = useStartExecution()
  const submitReport = useSubmitReviewReport()
  const userId = useAuthStore((state) => state.user?.id)
  const [evaluations, setEvaluations] = useState<Record<string, GateEvaluationResponse>>({})
  const [reportForm, setReportForm] = useState<Record<string, SubmitReviewReportRequest>>({})
  const [executionForm, setExecutionForm] = useState<{
    role: string
    agentId: string
    purpose: ExecutionPurpose | ''
    prompt: string
    artifactIds: string
  }>({ role: '', agentId: '', purpose: '', prompt: '', artifactIds: '' })

  const task = taskQuery.data
  const reviews = reviewsQuery.data ?? []
  const validations = validationsQuery.data ?? []
  const executions = executionsQuery.data?.items ?? []
  const executableRoles =
    task?.task_roles.filter((role) =>
      role.members.some(
        (member) => member.status === 'active' && member.actor_ref.kind === 'agent',
      ),
    ) ?? []
  const executableAgents =
    executableRoles
      .find((role) => role.role === executionForm.role)
      ?.members.filter(
        (member) => member.status === 'active' && member.actor_ref.kind === 'agent',
      )
      .map((member) => member.actor_ref.id) ?? []
  const reviewerRole = task?.task_roles.find((role) => role.role === 'reviewer')
  const canStartReview = Boolean(
    task?.lifecycle.state === 'active' &&
      userId &&
      reviewerRole?.members.some(
        (member) =>
          member.status === 'active' &&
          member.actor_ref.kind === 'human' &&
          member.actor_ref.id === userId,
      ),
  )

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

  async function beginReview() {
    if (!task) return
    try {
      await startReview.mutateAsync({
        taskId: task.id,
        body: { workspace_id: task.workspace?.id ?? null },
      })
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Review Execution could not start'))
    }
  }

  async function beginExecution() {
    if (!task || !executionForm.role || !executionForm.agentId || !executionForm.purpose) return
    const body: StartExecutionRequest = {
      agent_id: executionForm.agentId,
      role: executionForm.role,
      purpose: executionForm.purpose,
      prompt: executionForm.prompt,
      input_artifact_ids: executionForm.artifactIds
        .split('\n')
        .map((value) => value.trim())
        .filter(Boolean),
    }
    try {
      await startExecution.mutateAsync({ taskId: task.id, body })
      setExecutionForm({ role: '', agentId: '', purpose: '', prompt: '', artifactIds: '' })
      toast.success('Execution started with the selected Agent and TaskRole')
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Execution could not start'))
    }
  }

  async function sendReviewReport(executionId: string) {
    const body = reportForm[executionId]
    if (!body) return
    try {
      await submitReport.mutateAsync({ executionId, body })
      setReportForm((current) => {
        const next = { ...current }
        delete next[executionId]
        return next
      })
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'ReviewReport could not be submitted'))
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
            {task.description ? <p className="mt-2 whitespace-pre-wrap text-sm text-muted-foreground">{task.description}</p> : null}
          </div>
          <Badge variant="outline">{task.lifecycle.state.replaceAll('_', ' ')}</Badge>
          <Badge variant="outline">{task.task_type}</Badge>
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
        </div>
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
            <h2 className="font-semibold">TaskRole memberships</h2>
            {(task.task_roles ?? []).length === 0 ? <p className="text-sm text-muted-foreground">No TaskRoles are defined.</p> : null}
            {(task.task_roles ?? []).map((role) => (
              <div key={role.id} className="space-y-1 rounded-md bg-muted/30 p-3">
                <p className="font-medium">{role.role}</p>
                {role.members.map((member) => (
                  <p key={member.id} className="font-mono text-xs text-muted-foreground">
                    {member.actor_ref.kind}:{member.actor_ref.id} · {member.status} · membership {member.id} v{String(member.version)}
                  </p>
                ))}
              </div>
            ))}
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

      {activeTab === 'executions' ? (
        <section className="space-y-3 rounded-lg border p-4">
          <h2 className="font-semibold">Executions</h2>
          <p className="text-xs text-muted-foreground">Every start names the exact Agent, active TaskRole, purpose, prompt, and Artifact inputs.</p>
          <div className="grid gap-3 rounded-md border p-3 md:grid-cols-2">
            <label className="space-y-1 text-sm">
              <span>TaskRole</span>
              <select
                className="h-9 w-full rounded-md border bg-background px-2"
                value={executionForm.role}
                onChange={(event) =>
                  setExecutionForm((current) => ({ ...current, role: event.target.value, agentId: '' }))
                }
              >
                <option value="">Select a TaskRole</option>
                {executableRoles.map((role) => <option key={role.id} value={role.role}>{role.role}</option>)}
              </select>
            </label>
            <label className="space-y-1 text-sm">
              <span>Agent membership</span>
              <select
                className="h-9 w-full rounded-md border bg-background px-2"
                value={executionForm.agentId}
                onChange={(event) => setExecutionForm((current) => ({ ...current, agentId: event.target.value }))}
              >
                <option value="">Select an active Agent membership</option>
                {executableAgents.map((agentId) => <option key={agentId} value={agentId}>{agentId}</option>)}
              </select>
            </label>
            <label className="space-y-1 text-sm">
              <span>Execution purpose</span>
              <select
                className="h-9 w-full rounded-md border bg-background px-2"
                value={executionForm.purpose}
                onChange={(event) => setExecutionForm((current) => ({ ...current, purpose: event.target.value as ExecutionPurpose | '' }))}
              >
                <option value="">Select a purpose</option>
                <option value="plan">Plan</option>
                <option value="implement">Implement</option>
                <option value="review">Review</option>
                <option value="validate">Validate</option>
                <option value="investigate">Investigate</option>
                <option value="orchestrate">Orchestrate</option>
                <option value="general">General</option>
              </select>
            </label>
            <label className="space-y-1 text-sm">
              <span>Exact Artifact input IDs (one per line)</span>
              <Textarea
                aria-label="Exact Artifact input IDs"
                value={executionForm.artifactIds}
                onChange={(event) => setExecutionForm((current) => ({ ...current, artifactIds: event.target.value }))}
                placeholder="Artifact IDs"
              />
            </label>
            <label className="space-y-1 text-sm md:col-span-2">
              <span>Execution prompt</span>
              <Textarea
                aria-label="Execution prompt"
                value={executionForm.prompt}
                onChange={(event) => setExecutionForm((current) => ({ ...current, prompt: event.target.value }))}
                placeholder="Task-specific instructions for the selected Agent"
              />
            </label>
            <div className="md:col-span-2">
              <Button
                size="sm"
                disabled={startExecution.isPending || !executionForm.role || !executionForm.agentId || !executionForm.purpose || !executionForm.prompt.trim()}
                onClick={() => void beginExecution()}
              >
                Start exact Agent Execution
              </Button>
            </div>
          </div>
          {executions.map((execution) => (
            <article key={execution.id} className="space-y-1 rounded-md border p-3 text-sm">
              <div className="flex flex-wrap items-center gap-2">
                <Badge variant="outline">{execution.status}</Badge>
                <span>role: {execution.role}</span>
                <span>purpose: {execution.purpose ?? 'general'}</span>
                <code className="ml-auto text-xs">{execution.id}</code>
              </div>
              <p className="text-xs text-muted-foreground">Actor: {execution.actor_ref?.kind ?? 'unavailable'}:{execution.actor_ref?.id ?? 'unavailable'} · Harness session: {execution.harness_session_id ?? 'none'} · Workspace: {execution.workspace_id ?? 'none'}</p>
            </article>
          ))}
          {executions.length === 0 ? <p className="text-sm text-muted-foreground">No Execution records are available.</p> : null}
        </section>
      ) : null}

      {activeTab === 'review' ? (
        <section className="space-y-4">
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border p-4">
            <div>
              <h2 className="font-semibold">Review Executions</h2>
              <p className="text-xs text-muted-foreground">Formal Review requires Execution role reviewer and purpose review.</p>
            </div>
            <Button size="sm" disabled={!canStartReview || startReview.isPending} onClick={() => void beginReview()}>
              Start Human Review Execution
            </Button>
          </div>
          {reviewsQuery.isLoading ? <p className="text-sm text-muted-foreground">Loading Review Executions…</p> : null}
          {reviews.map(({ execution, report }) => (
            <ReviewExecutionCard
              key={execution.id}
              review={{ execution, report }}
              validations={validations}
              form={reportForm[execution.id]}
              onFormChange={(form) => setReportForm((current) => ({ ...current, [execution.id]: form }))}
              onSubmit={() => void sendReviewReport(execution.id)}
              submitting={submitReport.isPending}
            />
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

function ReviewExecutionCard({
  review,
  validations,
  form,
  onFormChange,
  onSubmit,
  submitting,
}: {
  review: ReviewExecutionResponse
  validations: import('@/types/generated').ValidationRunResponse[]
  form: SubmitReviewReportRequest | undefined
  onFormChange: (form: SubmitReviewReportRequest) => void
  onSubmit: () => void
  submitting: boolean
}) {
  const { execution, report } = review
  const ownsExecution = execution.actor_ref?.kind === 'human'
  const canSubmit = ownsExecution && execution.status === 'running' && !report
  const currentForm = form ?? {
    verdict: 'pass',
    summary: '',
    criteria: [],
    findings: [],
    questions: [],
    evidence_ids: [],
    artifact_ids: [],
  }
  const selectedEvidence = new Set(currentForm.evidence_ids)

  return (
    <article className="space-y-3 rounded-lg border p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{execution.status}</Badge>
        <Badge variant="outline">{execution.role} · {execution.purpose ?? 'general'}</Badge>
        <code className="ml-auto text-xs">Execution {execution.id}</code>
      </div>
      <p className="text-xs text-muted-foreground">Actor {execution.actor_ref?.kind ?? 'unknown'}:{execution.actor_ref?.id ?? 'unknown'} · Workspace {execution.workspace_id ?? 'none'} · base {execution.before_sha ?? 'none'} · head {execution.after_sha ?? 'none'}</p>
      {report ? (
        <div className="space-y-2 rounded bg-muted/40 p-3">
          <div className="flex flex-wrap gap-2"><Badge>ReviewReport</Badge><code className="text-xs">Artifact {report.id}</code></div>
          <p className="text-sm">{report.content ?? 'Report content is not available.'}</p>
        </div>
      ) : null}
      {canSubmit ? (
        <div className="space-y-3 rounded bg-muted/30 p-3">
          <label className="block space-y-1 text-sm">
            <span>Verdict</span>
            <select
              className="h-9 w-full rounded-md border bg-background px-2"
              value={currentForm.verdict}
              onChange={(event) => onFormChange({ ...currentForm, verdict: event.target.value as SubmitReviewReportRequest['verdict'] })}
            >
              <option value="pass">Pass</option><option value="request_changes">Request changes</option><option value="questions">Questions</option>
            </select>
          </label>
          <Textarea aria-label="Review summary" value={currentForm.summary} onChange={(event) => onFormChange({ ...currentForm, summary: event.target.value })} placeholder="Summary for this ReviewReport" />
          <div className="space-y-1">
            <p className="text-xs font-medium">Pin exact Validation Evidence</p>
            {validations.flatMap((run) => run.evidence_ids.map((id) => (
              <label key={id} className="flex items-center gap-2 font-mono text-xs">
                <input type="checkbox" checked={selectedEvidence.has(id)} onChange={(event) => onFormChange({ ...currentForm, evidence_ids: event.target.checked ? [...currentForm.evidence_ids, id] : currentForm.evidence_ids.filter((value) => value !== id) })} />
                {id} · ValidationRun {run.id}
              </label>
            )))}
            {validations.every((run) => run.evidence_ids.length === 0) ? <p className="text-xs text-muted-foreground">No Evidence facts are available to pin.</p> : null}
          </div>
          <Input aria-label="Exact Artifact IDs" value={currentForm.artifact_ids.join('\n')} onChange={(event) => onFormChange({ ...currentForm, artifact_ids: event.target.value.split('\n').map((value) => value.trim()).filter(Boolean) })} placeholder="One exact Artifact ID per line" />
          <Button size="sm" disabled={!currentForm.summary.trim() || submitting} onClick={onSubmit}>Submit ReviewReport for this Execution</Button>
        </div>
      ) : null}
    </article>
  )
}
