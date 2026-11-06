import { useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useRequestRework } from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  ActorRef,
  ReviewExecutionResponse,
  Task,
  TaskRoleResponse,
} from '@/types/generated'

type ReworkSubject = {
  verdict: string
  reviewedWorkUnitId: string
  reviewedWorkUnitVersion: number
}

function parseReworkSubject(
  review: ReviewExecutionResponse,
): ReworkSubject | null {
  if (!review.report?.content) return null
  try {
    const parsed = JSON.parse(review.report.content) as {
      verdict?: unknown
      subject?: {
        reviewed_work_unit_id?: unknown
        reviewed_work_unit_version?: unknown
      }
    }
    if (
      parsed.verdict !== 'request_changes' ||
      typeof parsed.subject?.reviewed_work_unit_id !== 'string' ||
      typeof parsed.subject?.reviewed_work_unit_version !== 'number'
    ) {
      return null
    }
    return {
      verdict: parsed.verdict,
      reviewedWorkUnitId: parsed.subject.reviewed_work_unit_id,
      reviewedWorkUnitVersion: parsed.subject.reviewed_work_unit_version,
    }
  } catch {
    return null
  }
}

function actorKey(actor: ActorRef | null | undefined): string {
  return actor ? `${actor.kind}:${actor.id}` : ''
}

function parseActor(value: string): ActorRef | null {
  if (!value) return null
  const [kind, ...id] = value.split(':')
  if ((kind !== 'human' && kind !== 'agent') || id.length === 0) return null
  return { kind, id: id.join(':') }
}

function activeMembers(role: TaskRoleResponse | undefined): ActorRef[] {
  return (role?.members ?? [])
    .filter((member) => member.status === 'active')
    .map((member) => member.actor_ref)
}

export function TaskReworkPanel({
  task,
  review,
  onChanged,
}: {
  task: Task
  review: ReviewExecutionResponse
  onChanged: () => Promise<unknown>
}) {
  const subject = useMemo(() => parseReworkSubject(review), [review])
  const requestRework = useRequestRework()
  const implementerRoles = task.task_roles.filter(
    (role) => role.role !== 'reviewer',
  )
  const [correctionTitle, setCorrectionTitle] = useState('Correction')
  const [correctionScope, setCorrectionScope] = useState(
    'Apply exactly the changes requested by the ReviewReport.',
  )
  const [correctionRole, setCorrectionRole] = useState(
    implementerRoles.find((role) => role.role === 'implementer')?.role ??
      implementerRoles[0]?.role ??
      '',
  )
  const [correctionActor, setCorrectionActor] = useState('')
  const [reviewTitle, setReviewTitle] = useState('Review correction')
  const [reviewScope, setReviewScope] = useState(
    'Review the exact correction result against the requested changes.',
  )
  const [reviewActor, setReviewActor] = useState('')
  const requestIdentity = useRef<{ fingerprint: string; key: string } | null>(null)

  if (!subject) return null

  const correctionMembers = activeMembers(
    task.task_roles.find((role) => role.role === correctionRole),
  )
  const reviewerMembers = activeMembers(
    task.task_roles.find((role) => role.role === 'reviewer'),
  )

  async function submit() {
    if (!review.report || !subject) return
    if (!correctionTitle.trim() || !correctionScope.trim() || !correctionRole) {
      toast.error('Correction title, scope, and role are required')
      return
    }
    try {
      const request = {
        expected_task_version: task.version,
        expected_lifecycle_version: task.lifecycle.version,
        review_report_artifact_id: review.report.id,
        reviewed_work_unit_id: subject.reviewedWorkUnitId,
        reviewed_work_unit_version: subject.reviewedWorkUnitVersion,
        correction_title: correctionTitle.trim(),
        correction_scope: correctionScope.trim(),
        correction_role: correctionRole,
        correction_actor: parseActor(correctionActor),
        review_title: reviewTitle.trim() || 'Review correction',
        review_scope:
          reviewScope.trim() ||
          'Review the exact correction result against the requested changes.',
        review_role: 'reviewer',
        review_actor: parseActor(reviewActor),
      }
      const fingerprint = JSON.stringify(request)
      const idempotencyKey =
        requestIdentity.current?.fingerprint === fingerprint
          ? requestIdentity.current.key
          : crypto.randomUUID()
      requestIdentity.current = { fingerprint, key: idempotencyKey }
      const result = await requestRework.mutateAsync({
        taskId: task.id,
        body: {
          ...request,
          idempotency_key: idempotencyKey,
        },
      })
      toast.success(
        result.replayed
          ? 'Existing correction and review round replayed'
          : `Correction and review round created · ${String(result.rounds_remaining)} rounds remaining`,
      )
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not request rework'))
    }
  }

  return (
    <div className="space-y-3 rounded-md border border-amber-300 bg-amber-50/50 p-3">
      <div>
        <h4 className="text-sm font-semibold">Request correction and new review</h4>
        <p className="text-xs text-muted-foreground">
          The reviewed WorkUnit {subject.reviewedWorkUnitId} v
          {String(subject.reviewedWorkUnitVersion)} stays unchanged. This creates a
          correction WorkUnit and a new reviewer WorkUnit with an exact result subject.
        </p>
      </div>
      <div className="grid gap-2 md:grid-cols-2">
        <label className="space-y-1 text-xs">
          <span>Correction title</span>
          <Input
            value={correctionTitle}
            onChange={(event) => setCorrectionTitle(event.target.value)}
          />
        </label>
        <label className="space-y-1 text-xs">
          <span>Correction TaskRole</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={correctionRole}
            onChange={(event) => {
              setCorrectionRole(event.target.value)
              setCorrectionActor('')
            }}
          >
            {implementerRoles.map((role) => (
              <option key={role.id} value={role.role}>
                {role.role}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1 text-xs md:col-span-2">
          <span>Correction scope</span>
          <Textarea
            value={correctionScope}
            onChange={(event) => setCorrectionScope(event.target.value)}
          />
        </label>
        <label className="space-y-1 text-xs">
          <span>Correction Actor (optional)</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={correctionActor}
            onChange={(event) => setCorrectionActor(event.target.value)}
          >
            <option value="">Leave allocation pending</option>
            {correctionMembers.map((actor) => (
              <option key={actorKey(actor)} value={actorKey(actor)}>
                {actor.kind}:{actor.id}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1 text-xs">
          <span>Review title</span>
          <Input
            value={reviewTitle}
            onChange={(event) => setReviewTitle(event.target.value)}
          />
        </label>
        <label className="space-y-1 text-xs md:col-span-2">
          <span>Review scope</span>
          <Textarea
            value={reviewScope}
            onChange={(event) => setReviewScope(event.target.value)}
          />
        </label>
        <label className="space-y-1 text-xs">
          <span>Reviewer Actor (optional)</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={reviewActor}
            onChange={(event) => setReviewActor(event.target.value)}
          >
            <option value="">Leave reviewer allocation pending</option>
            {reviewerMembers.map((actor) => (
              <option key={actorKey(actor)} value={actorKey(actor)}>
                {actor.kind}:{actor.id}
              </option>
            ))}
          </select>
        </label>
      </div>
      <Button size="sm" disabled={requestRework.isPending} onClick={() => void submit()}>
        {requestRework.isPending ? 'Creating…' : 'Request correction + new review'}
      </Button>
    </div>
  )
}
