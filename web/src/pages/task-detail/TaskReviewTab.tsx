import { ArrowCounterClockwise, Spinner } from '@phosphor-icons/react'
import { useState, type FormEvent } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { WorkflowExceptionPanel } from '@/components/task-detail/workflow-exception-panel'
import { cn } from '@/lib/cn'
import type {
  RecoveryAction,
  ReviewExecutionResponse,
  SubmitReviewReportRequest,
  Task,
  ValidationRunResponse,
  WorkflowExceptionAction,
} from '@/types/generated'
import { formatDate } from './utils'

const validationStatusClasses: Record<ValidationRunResponse['status'], string> = {
  running: 'bg-amber-100 text-amber-900 dark:bg-amber-900/30 dark:text-amber-300',
  passed: 'bg-emerald-100 text-emerald-900 dark:bg-emerald-900/30 dark:text-emerald-300',
  failed: 'bg-red-100 text-red-900 dark:bg-red-900/30 dark:text-red-300',
  error: 'bg-red-100 text-red-900 dark:bg-red-900/30 dark:text-red-300',
  cancelled: 'bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-400',
  stale: 'bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-400',
}

type TaskReviewTabProps = {
  task: Task
  reviews: ReviewExecutionResponse[]
  validations: ValidationRunResponse[]
  reviewsLoading: boolean
  validationsLoading: boolean
  reviewStartPending: boolean
  reportSubmitPending: boolean
  recoverPending: boolean
  cancelPending: boolean
  terminal: boolean
  onStartReview: () => void
  onSubmitReport: (executionId: string, body: SubmitReviewReportRequest) => void
  onRecover: (action: RecoveryAction, input?: { reason?: string; context?: string }) => void
  onOpenWorkflowExceptionAction: (action: WorkflowExceptionAction) => void
  onCancelTask: () => void
}

export function TaskReviewTab({
  task,
  reviews,
  validations,
  reviewsLoading,
  validationsLoading,
  reviewStartPending,
  reportSubmitPending,
  recoverPending,
  cancelPending,
  terminal,
  onStartReview,
  onSubmitReport,
  onRecover,
  onOpenWorkflowExceptionAction,
  onCancelTask,
}: TaskReviewTabProps) {
  const canAct = task.status === 'review'
  const workflowExceptionActions = task.workflow_exception?.actions ?? []
  const chronologicalReviews = [...reviews].sort((a, b) =>
    b.execution.created_at.localeCompare(a.execution.created_at),
  )
  const chronologicalValidations = [...validations].sort((a, b) =>
    b.created_at.localeCompare(a.created_at),
  )

  return (
    <div className="space-y-4">
      <WorkflowExceptionPanel
        task={task}
        actions={workflowExceptionActions}
        recoverPending={recoverPending}
        terminal={terminal}
        cancelPending={cancelPending}
        onRecover={onRecover}
        onOpenInteractive={onOpenWorkflowExceptionAction}
        onCancelTask={onCancelTask}
      />

      <section className="space-y-3 rounded-lg border p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold">Cognitive reviews</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Each report belongs to the exact reviewer Execution shown below.
            </p>
          </div>
          <Button size="sm" variant="outline" disabled={!canAct || reviewStartPending} onClick={onStartReview}>
            {reviewStartPending ? (
              <Spinner className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <ArrowCounterClockwise className="h-3.5 w-3.5" />
            )}
            Start Human review
          </Button>
        </div>

        {reviewsLoading ? (
          <p className="text-sm text-muted-foreground">Loading review Executions…</p>
        ) : chronologicalReviews.length === 0 ? (
          <p className="text-sm text-muted-foreground">No reviewer Execution is recorded.</p>
        ) : (
          <div className="space-y-3">
            {chronologicalReviews.map(({ execution, report }) => {
              const activeHumanReview =
                execution.status === 'running' && execution.actor_ref?.kind === 'human'
              const content = parseReport(report?.content)
              return (
                <article key={execution.id} className="space-y-3 rounded-md border p-3">
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge variant="outline">{execution.status}</Badge>
                    <Badge variant="outline">{execution.purpose ?? 'purpose missing'}</Badge>
                    <span className="font-mono text-xs text-muted-foreground">
                      Execution {execution.id}
                    </span>
                    <span className="ml-auto text-xs text-muted-foreground">
                      {formatDate(execution.created_at)}
                    </span>
                  </div>
                  <div className="grid gap-1 text-xs text-muted-foreground sm:grid-cols-2">
                    <p>Actor: {execution.actor_ref?.kind ?? 'unresolved'} / {execution.actor_ref?.id ?? 'unknown'}</p>
                    <p>Workspace: {execution.workspace_id ?? 'none'}</p>
                    <p>Base commit: {execution.before_sha ?? 'none'}</p>
                    <p>Head commit: {execution.after_sha ?? 'none'}</p>
                  </div>

                  {report ? (
                    <div className="space-y-2 rounded-md bg-muted/30 p-3">
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge variant="outline">ReviewReport · {content?.verdict ?? 'verdict unavailable'}</Badge>
                        <span className="font-mono text-xs text-muted-foreground">Artifact {report.id}</span>
                      </div>
                      <p className="whitespace-pre-wrap text-sm">{content?.summary ?? report.content ?? 'Report content unavailable.'}</p>
                      {content?.findings?.length ? (
                        <ul className="list-disc space-y-1 pl-5 text-sm">
                          {content.findings.map((finding, index) => <li key={`${index}-${finding}`}>{finding}</li>)}
                        </ul>
                      ) : null}
                      {content?.questions?.length ? (
                        <div className="text-sm">
                          <p className="font-medium">Questions</p>
                          <ul className="list-disc pl-5">
                            {content.questions.map((question, index) => <li key={`${index}-${question}`}>{question}</li>)}
                          </ul>
                        </div>
                      ) : null}
                    </div>
                  ) : activeHumanReview ? (
                    <HumanReviewForm
                      disabled={reportSubmitPending}
                      onSubmit={(body) => onSubmitReport(execution.id, body)}
                    />
                  ) : (
                    <p className="text-xs text-muted-foreground">
                      This Execution has no ReviewReport Artifact yet.
                    </p>
                  )}
                </article>
              )
            })}
          </div>
        )}
      </section>

      <section className="space-y-3 rounded-lg border p-4">
        <div>
          <h2 className="text-sm font-semibold">Deterministic validation</h2>
          <p className="mt-1 text-xs text-muted-foreground">
            Validation results are independent of Review verdicts and are pinned to exact commits.
          </p>
        </div>
        {validationsLoading ? (
          <p className="text-sm text-muted-foreground">Loading ValidationRuns…</p>
        ) : chronologicalValidations.length === 0 ? (
          <p className="text-sm text-muted-foreground">No deterministic ValidationRun is recorded.</p>
        ) : (
          <div className="space-y-2">
            {chronologicalValidations.map((run) => (
              <article key={run.id} className="space-y-2 rounded-md border p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge className={cn('border-transparent', validationStatusClasses[run.status])}>
                    {run.status}{run.exit_code === null ? '' : ` · exit ${run.exit_code}`}
                  </Badge>
                  <span className="font-medium">{run.check_identity}</span>
                  <span className="ml-auto font-mono text-xs text-muted-foreground">ValidationRun {run.id}</span>
                </div>
                <code className="block break-all rounded bg-muted p-2 text-xs">{run.command}</code>
                <div className="grid gap-1 text-xs text-muted-foreground sm:grid-cols-2">
                  <p>Workspace: {run.workspace_id}</p>
                  <p>Commit: <code>{run.commit_sha}</code></p>
                  <p className="break-all sm:col-span-2">Workspace snapshot: <code>{run.workspace_snapshot_digest}</code></p>
                  <p>Started: {formatDate(run.started_at)}</p>
                  <p>Finished: {formatDate(run.finished_at)}</p>
                </div>
                <p className="text-xs">Evidence: {run.evidence_ids.length ? run.evidence_ids.map((id) => <code key={id} className="mr-2">{id}</code>) : 'none'}</p>
                {run.validation_report_artifact_id ? (
                  <p className="text-xs">Validation report Artifact: <code>{run.validation_report_artifact_id}</code></p>
                ) : null}
                {run.logs_ref ? <p className="break-all text-xs text-muted-foreground">Logs: {run.logs_ref}</p> : null}
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  )
}

function HumanReviewForm({
  disabled,
  onSubmit,
}: {
  disabled: boolean
  onSubmit: (body: SubmitReviewReportRequest) => void
}) {
  const [verdict, setVerdict] = useState<SubmitReviewReportRequest['verdict']>('pass')
  const [summary, setSummary] = useState('')
  const [criteria, setCriteria] = useState('')
  const [findings, setFindings] = useState('')
  const [questions, setQuestions] = useState('')
  const [evidenceIds, setEvidenceIds] = useState('')
  const [artifactIds, setArtifactIds] = useState('')
  const [error, setError] = useState<string | null>(null)

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!summary.trim()) {
      setError('A summary is required for a complete ReviewReport.')
      return
    }
    const splitLines = (value: string) => value.split('\n').map((line) => line.trim()).filter(Boolean)
    const body: SubmitReviewReportRequest = {
      verdict,
      summary: summary.trim(),
      criteria: splitLines(criteria),
      findings: splitLines(findings),
      questions: splitLines(questions),
      evidence_ids: splitLines(evidenceIds),
      artifact_ids: splitLines(artifactIds),
    }
    onSubmit(body)
    setError(null)
  }

  return (
    <form className="space-y-3 rounded-md border border-dashed p-3" onSubmit={submit}>
      <p className="text-sm font-medium">Submit a report for this Human Review Execution</p>
      <div className="space-y-1">
        <Label htmlFor="review-verdict">Verdict</Label>
        <select id="review-verdict" className="h-9 w-full rounded-md border bg-background px-3 text-sm" value={verdict} onChange={(event) => setVerdict(event.target.value as SubmitReviewReportRequest['verdict'])}>
          <option value="pass">Pass</option>
          <option value="request_changes">Request changes</option>
          <option value="questions">Questions / escalation</option>
        </select>
      </div>
      <Field label="Summary" value={summary} onChange={setSummary} required />
      <Field label="Criteria (one per line)" value={criteria} onChange={setCriteria} />
      <Field label="Findings (one per line)" value={findings} onChange={setFindings} />
      <Field label="Questions (one per line)" value={questions} onChange={setQuestions} />
      <Field label="Exact Evidence IDs considered (one per line)" value={evidenceIds} onChange={setEvidenceIds} />
      <Field label="Exact Artifact IDs considered (one per line)" value={artifactIds} onChange={setArtifactIds} />
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
      <Button size="sm" type="submit" disabled={disabled}>
        {disabled ? <Spinner className="h-3.5 w-3.5 animate-spin" /> : null}
        Submit ReviewReport
      </Button>
    </form>
  )
}

function Field({
  label,
  value,
  onChange,
  required = false,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  required?: boolean
}) {
  return (
    <div className="space-y-1">
      <Label>{label}</Label>
      <Textarea required={required} value={value} onChange={(event) => onChange(event.target.value)} rows={label === 'Summary' ? 3 : 2} />
    </div>
  )
}

function parseReport(value: string | null | undefined): {
  verdict?: string
  summary?: string
  findings?: string[]
  questions?: string[]
} | null {
  if (!value) return null
  try {
    const result: unknown = JSON.parse(value)
    if (!result || typeof result !== 'object') return null
    const record = result as Record<string, unknown>
    return {
      verdict: typeof record.verdict === 'string' ? record.verdict : undefined,
      summary: typeof record.summary === 'string' ? record.summary : undefined,
      findings: Array.isArray(record.findings) ? record.findings.filter((item): item is string => typeof item === 'string') : undefined,
      questions: Array.isArray(record.questions) ? record.questions.filter((item): item is string => typeof item === 'string') : undefined,
    }
  } catch {
    return null
  }
}
