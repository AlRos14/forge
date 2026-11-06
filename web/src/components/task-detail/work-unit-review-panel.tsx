import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { useSubmitWorkUnitReviewReport } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  SubmitReviewReportRequest,
  ReviewSubjectResponse,
  Task,
  WorkUnitResponse,
} from '@/types/generated'

function initialReport(
  subject: ReviewSubjectResponse,
): SubmitReviewReportRequest {
  return {
    verdict: 'pass',
    summary: '',
    criteria: [],
    findings: [],
    questions: [],
    evidence_ids: [],
    artifact_ids: [],
    reviewed_work_unit_id: subject.reviewed_work_unit_id,
    reviewed_work_unit_version: subject.reviewed_work_unit_version,
  }
}

export function WorkUnitHumanReviewPanel({
  task,
  unit,
  currentUserId,
}: {
  task: Task
  unit: WorkUnitResponse
  currentUserId: string | undefined
}) {
  const subject = unit.review_subject
  const [report, setReport] = useState<SubmitReviewReportRequest | null>(() =>
    subject?.reviewed_work_unit_version == null ? null : initialReport(subject),
  )
  useEffect(() => {
    if (subject?.reviewed_work_unit_version != null) {
      setReport((current) => current ?? initialReport(subject))
    }
  }, [subject])
  const submitReview = useSubmitWorkUnitReviewReport()
  const reviewerRole = task.task_roles.find((role) => role.role === 'reviewer')
  const isAssignedReviewer = Boolean(
    currentUserId &&
      unit.assigned_actor?.kind === 'human' &&
      unit.assigned_actor.id === currentUserId &&
      reviewerRole?.members.some(
        (member) =>
          member.status === 'active' &&
          member.actor_ref.kind === 'human' &&
          member.actor_ref.id === currentUserId,
      ),
  )

  if (!isAssignedReviewer) return null

  if (!subject) {
    return (
      <article className="rounded-lg border border-amber-300 p-4 text-sm">
        <h3 className="font-semibold">{unit.title}</h3>
        <p className="mt-1 text-muted-foreground">
          This reviewer WorkUnit has no exact subject. Create a new reviewer WorkUnit and select
          the WorkUnit to inspect.
        </p>
      </article>
    )
  }

  if (subject.reviewed_work_unit_version == null || !unit.readiness.runnable) {
    return (
      <article className="rounded-lg border p-4 text-sm">
        <h3 className="font-semibold">{unit.title}</h3>
        <p className="mt-1 text-muted-foreground">
          Waiting for the exact result of WorkUnit {subject.reviewed_work_unit_id}
          {subject.reviewed_work_unit_version == null
            ? ''
            : ` v${String(subject.reviewed_work_unit_version)}`}.
          {unit.readiness.unsatisfied_dependency_ids.length
            ? ` Unresolved dependencies: ${unit.readiness.unsatisfied_dependency_ids.join(', ')}.`
            : ''}
        </p>
      </article>
    )
  }

  const exactSubject = subject
  const exactReviewVersion = exactSubject.reviewed_work_unit_version
  if (exactReviewVersion == null) return null
  if (!report) return null

  async function submit() {
    if (!report) return
    if (!report.summary.trim()) {
      toast.error('Review summary is required')
      return
    }
    try {
      const result = await submitReview.mutateAsync({
        workUnitId: unit.id,
        body: {
          expected_version: unit.version,
          report: {
            ...report,
            reviewed_work_unit_id: exactSubject.reviewed_work_unit_id,
            reviewed_work_unit_version: exactReviewVersion,
          },
        },
      })
      toast.success(`ReviewReport submitted · Execution ${result.review_execution.execution.id}`)
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'ReviewReport could not be submitted'))
    }
  }

  return (
    <article className="space-y-3 rounded-lg border border-primary/30 p-4">
      <div>
        <h3 className="font-semibold">Human review · {unit.title}</h3>
        <p className="mt-1 text-xs text-muted-foreground">
          Exact subject: WorkUnit {subject.reviewed_work_unit_id} v
          {String(subject.reviewed_work_unit_version)}
          {subject.result_execution_id ? ` · Execution ${subject.result_execution_id}` : ''}
          {subject.result_artifact_id ? ` · Artifact ${subject.result_artifact_id}` : ''}
          {subject.result_commit_sha ? ` · commit ${subject.result_commit_sha}` : ''}
        </p>
      </div>
      <label className="block space-y-1 text-sm">
        <span>Verdict</span>
        <select
          aria-label="Human review verdict"
          className="h-9 w-full rounded-md border bg-background px-2"
          value={report.verdict}
          onChange={(event) =>
            setReport({
              ...report,
              verdict: event.target.value as SubmitReviewReportRequest['verdict'],
            })
          }
        >
          <option value="pass">Pass</option>
          <option value="request_changes">Request changes</option>
          <option value="questions">Questions</option>
        </select>
      </label>
      <Textarea
        aria-label="Human review summary"
        value={report.summary}
        onChange={(event) => setReport({ ...report, summary: event.target.value })}
        placeholder="Summarize the review of this exact WorkUnit result"
      />
      <Textarea
        aria-label="Requested changes"
        value={report.findings.join('\n')}
        onChange={(event) =>
          setReport({
            ...report,
            findings: event.target.value.split('\n').map((item) => item.trim()).filter(Boolean),
          })
        }
        placeholder="Requested changes or findings, one per line"
      />
      <Button
        size="sm"
        disabled={!report.summary.trim() || submitReview.isPending}
        onClick={() => void submit()}
      >
        {submitReview.isPending ? 'Submitting…' : 'Submit Human ReviewReport'}
      </Button>
    </article>
  )
}
