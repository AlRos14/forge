import { useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import {
  useRequestRework,
  useTaskReworkAutomationQuery,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import { resolveActorIdentity } from '@/features/task-roles/ActorIdentity'
import type {
  ActorRef,
  AgentResponse,
  DaemonResponse,
  ProjectMemberResponse,
  ReviewExecutionResponse,
  Task,
  TaskRoleResponse,
  WorkUnitResponse,
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
  agents = [],
  members = [],
  daemons = [],
  workUnits = [],
}: {
  task: Task
  review: ReviewExecutionResponse
  onChanged: () => Promise<unknown>
  agents?: AgentResponse[]
  members?: ProjectMemberResponse[]
  daemons?: DaemonResponse[]
  workUnits?: WorkUnitResponse[]
}) {
  const subject = useMemo(() => parseReworkSubject(review), [review])
  const requestRework = useRequestRework()
  const automationQuery = useTaskReworkAutomationQuery(task.id)
  const reviewerRole = task.task_roles.find((role) => role.role === review.execution.role)
  const reworkPolicy = reviewerRole?.configuration.rework
  const allowedRoles = task.task_roles.filter((role) =>
    reworkPolicy?.allowed_destination_role_ids.includes(role.id),
  )
  const [correctionTitle, setCorrectionTitle] = useState('Correction')
  const [correctionScope, setCorrectionScope] = useState(
    'Apply exactly the changes requested by the ReviewReport.',
  )
  const [correctionRole, setCorrectionRole] = useState(
    allowedRoles[0]?.id ??
      '',
  )
  const [correctionActor, setCorrectionActor] = useState(() => {
    const selection = reworkPolicy?.correction_actor
    return selection?.kind === 'explicit'
      ? actorKey(selection.actor_ref)
      : ''
  })
  const [reviewTitle, setReviewTitle] = useState('Review correction')
  const [reviewScope, setReviewScope] = useState(
    'Review the exact correction result against the requested changes.',
  )
  const [reviewActor, setReviewActor] = useState('')
  const requestIdentity = useRef<{ fingerprint: string; key: string } | null>(null)

  if (!subject) return null

  const automationIntent = automationQuery.data?.items.find(
    (intent) => intent.review_report_artifact_id === review.report?.id,
  )

  const correctionMembers = activeMembers(
    task.task_roles.find((role) => role.role === correctionRole),
  )
  const nextReviewRole = task.task_roles.find(
    (role) => role.id === reworkPolicy?.next_review_role_id,
  )
  const reviewerMembers = activeMembers(nextReviewRole)
  const configuredCorrectionActor = reworkPolicy?.correction_actor
  const correctionActorRequired = !configuredCorrectionActor
  const configuredReviewActor = reworkPolicy?.review_actor
  const reviewedUnit = workUnits.find((unit) => unit.id === subject.reviewedWorkUnitId)
  const actorDisplay = (actor: ActorRef) => {
    const identity = resolveActorIdentity(actor, agents, members, daemons)
    return `${identity.name} · ${identity.context}`
  }

  async function submit() {
    if (!review.report || !subject) return
    if (
      !correctionTitle.trim() ||
      !correctionScope.trim() ||
      !correctionRole ||
      !nextReviewRole ||
      (correctionActorRequired && !correctionActor) ||
      (!configuredReviewActor && !reviewActor)
    ) {
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
        correction_role_id: correctionRole,
        correction_actor: correctionActorRequired ? parseActor(correctionActor) : null,
        review_title: reviewTitle.trim() || 'Review correction',
        review_scope:
          reviewScope.trim() ||
          'Review the exact correction result against the requested changes.',
        review_actor: configuredReviewActor ?? parseActor(reviewActor),
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
          The reviewed WorkUnit{' '}
          <span className="font-medium text-foreground">
            {reviewedUnit?.title ?? subject.reviewedWorkUnitId}
          </span>{' '}
          v{String(subject.reviewedWorkUnitVersion)} stays unchanged. This creates a correction
          WorkUnit and a new reviewer WorkUnit with an exact result subject.
        </p>
      </div>
      {automationIntent?.state === 'pending' ? (
        <p role="status" className="rounded bg-muted/40 p-2 text-xs text-muted-foreground">
          Automatic correction is durably queued. It will create the correction and next review after the Task resumes and the saved authority is revalidated.
        </p>
      ) : null}
      {automationIntent?.state === 'blocked' ? (
        <p role="status" className="rounded border border-amber-400 bg-amber-50 p-2 text-xs text-amber-950">
          Automatic correction needs a human decision: {automationIntent.block_reason || 'the saved route is no longer eligible'}.
        </p>
      ) : null}
      {automationIntent?.state === 'cancelled' ? (
        <p role="status" className="rounded bg-muted/40 p-2 text-xs text-muted-foreground">
          Automatic correction was cancelled with the Task. No new WorkUnits were created.
        </p>
      ) : null}
      {automationIntent?.state === 'completed' ? (
        <p role="status" className="rounded bg-muted/40 p-2 text-xs text-muted-foreground">
          Automatic correction round {String(automationIntent.rework_round)} created and linked to the exact ReviewReport.
        </p>
      ) : null}
      {!reviewerRole?.configuration.capabilities.includes('request_rework') || !reworkPolicy ? (
        <p role="status" className="rounded bg-muted/40 p-2 text-xs text-muted-foreground">
          This ReviewReport was produced by a role without request_rework routing. A Project owner or admin must configure an authorized route before a correction pair can be created.
        </p>
      ) : null}
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
            {allowedRoles.map((role) => (
              <option key={role.id} value={role.id}>
                {role.configuration.display_label || role.role}
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
          <span>Correction Actor{correctionActorRequired ? '' : ' (configured)'}</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={correctionActor}
            disabled={!correctionActorRequired}
            onChange={(event) => setCorrectionActor(event.target.value)}
          >
            <option value="">Choose an active member of the correction role</option>
            {correctionMembers.map((actor) => (
              <option key={actorKey(actor)} value={actorKey(actor)}>
                {actorDisplay(actor)}
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
          <span>Next review Actor{configuredReviewActor ? ' (configured)' : ''}</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={reviewActor}
            disabled={Boolean(configuredReviewActor)}
            onChange={(event) => setReviewActor(event.target.value)}
          >
            <option value="">Choose an active member of the next review role</option>
            {reviewerMembers.map((actor) => (
              <option key={actorKey(actor)} value={actorKey(actor)}>
                {actorDisplay(actor)}
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
