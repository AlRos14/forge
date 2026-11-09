import { useMemo, useState, type FormEvent } from 'react'
import { toast } from 'sonner'
import {
  useCreateTaskQualityRequest,
  useResolveTaskQualityRequest,
  useTaskQualityRequestsQuery,
} from '@/api/hooks'
import { ActorIdentity } from '@/features/task-roles/ActorIdentity'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  AgentResponse,
  DaemonResponse,
  ProjectMemberResponse,
  Task,
  TaskCoordinationOverviewResponse,
  TaskQualityCategory,
  TaskQualityRequestResponse,
  TaskQualityResolution,
  TaskQualityState,
} from '@/types/generated'

const categoryOptions: Array<{ value: TaskQualityCategory; label: string; target: string }> = [
  { value: 'delivery_correction', label: 'Delivery correction', target: 'Implementation role' },
  { value: 'plan_revision', label: 'Plan or coordination issue', target: 'Orchestrator role' },
  { value: 'specialist', label: 'Specialist input', target: 'Specialist role' },
  { value: 'human_decision', label: 'Human decision', target: 'Named person' },
]

const stateLabels: Record<TaskQualityState, string> = {
  requested: 'Requested',
  notified: 'Handoff recorded',
  work_created: 'Work item created',
  running: 'In progress',
  resolved: 'Resolved',
  requires_human: 'Needs a human decision',
  cancelled: 'Cancelled',
}

function roleLabel(role: TaskCoordinationOverviewResponse['team'][number]) {
  return role.display_label?.trim() || role.role.replaceAll('_', ' ')
}

function isTerminal(state: TaskQualityState) {
  return state === 'resolved' || state === 'cancelled'
}

export function TaskQualityRequestsPanel({
  task,
  coordination,
  agents,
  members,
  daemons,
}: {
  task: Task
  coordination: TaskCoordinationOverviewResponse
  agents: AgentResponse[]
  members: ProjectMemberResponse[]
  daemons: DaemonResponse[]
}) {
  const query = useTaskQualityRequestsQuery(task.id)
  const createRequest = useCreateTaskQualityRequest()
  const resolveRequest = useResolveTaskQualityRequest()
  const [category, setCategory] = useState<TaskQualityCategory>('delivery_correction')
  const [roleId, setRoleId] = useState('')
  const [actorKey, setActorKey] = useState('')
  const [summary, setSummary] = useState('')
  const [findings, setFindings] = useState('')
  const [resolutionCondition, setResolutionCondition] = useState('')
  const [sourceArtifactId, setSourceArtifactId] = useState('')
  const [actionError, setActionError] = useState<string | null>(null)

  const roleOptions = useMemo(() => {
    const activeRoles = coordination.team.filter((role) =>
      role.members.some((member) => member.status === 'active'),
    )
    const eligibleRoles =
      category === 'plan_revision'
        ? activeRoles.filter((role) => role.capabilities.includes('orchestration'))
        : activeRoles
    return eligibleRoles.map((role) => ({ value: role.id, label: roleLabel(role) }))
  }, [category, coordination.team])
  const humanOptions = useMemo(() => {
    const unique = new Map<string, { value: string; label: string }>()
    for (const role of coordination.team) {
      for (const member of role.members) {
        if (member.status !== 'active' || member.actor_ref.kind !== 'human') continue
        unique.set(member.actor_ref.id, {
          value: member.actor_ref.id,
          label: member.display_name,
        })
      }
    }
    return [...unique.values()]
  }, [coordination.team])

  const canSubmit =
    summary.trim().length > 0 &&
    resolutionCondition.trim().length > 0 &&
    (category === 'human_decision' ? Boolean(actorKey) : Boolean(roleId))

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!canSubmit) return
    setActionError(null)
    const human = category === 'human_decision'
    try {
      await createRequest.mutateAsync({
        taskId: task.id,
        body: {
          source_artifact_id: sourceArtifactId.trim() || null,
          source_execution_id: null,
          category,
          target_kind: human
            ? 'human'
            : category === 'plan_revision'
              ? 'orchestrator'
              : category === 'specialist'
                ? 'specialist'
                : 'implementation',
          target_task_role_id: human ? null : roleId,
          target_actor: human ? { kind: 'human', id: actorKey } : null,
          summary: summary.trim(),
          findings: findings
            .split('\n')
            .map((finding) => finding.trim())
            .filter(Boolean),
          resolution_condition: resolutionCondition.trim(),
          idempotency_key: crypto.randomUUID(),
        },
      })
      toast.success('Quality request recorded')
      setSummary('')
      setFindings('')
      setResolutionCondition('')
      setSourceArtifactId('')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Quality request could not be recorded')
      setActionError(message)
      toast.error(message)
    }
  }

  async function resolve(item: TaskQualityRequestResponse, resolution: TaskQualityResolution) {
    setActionError(null)
    try {
      await resolveRequest.mutateAsync({
        taskId: task.id,
        requestId: item.id,
        body: {
          expected_version: item.version,
          resolution,
          idempotency_key: crypto.randomUUID(),
        },
      })
      toast.success('Quality request updated')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Quality request could not be updated')
      setActionError(message)
      toast.error(message)
    }
  }

  return (
    <section
      className="min-w-0 space-y-4 rounded-lg border p-4"
      aria-labelledby="quality-requests-title"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 id="quality-requests-title" className="font-semibold">
            Review and quality requests
          </h3>
          <p className="mt-1 text-sm text-muted-foreground">
            Route a delivery correction, plan revision, specialist request, or human decision. A
            handoff does not by itself create or start a WorkUnit.
          </p>
        </div>
        <Badge variant="outline">
          {coordination.quality_request_count} / {coordination.max_quality_requests} requests
        </Badge>
      </div>

      {query.isLoading ? (
        <p className="text-sm text-muted-foreground">Loading quality requests…</p>
      ) : null}
      {query.isError ? (
        <div
          role="alert"
          className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          Quality requests could not be loaded.{' '}
          <Button size="sm" variant="outline" onClick={() => void query.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      {query.data?.items.length ? (
        <ul className="space-y-3">
          {query.data.items.map((item) => {
            const targetRole = coordination.team.find(
              (role) => role.id === item.target_task_role_id,
            )
            return (
              <li key={item.id} className="min-w-0 space-y-3 rounded-md border p-3">
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div className="min-w-0">
                    <p className="break-words text-sm font-medium">{item.summary}</p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {categoryOptions.find((option) => option.value === item.category)?.label ??
                        item.category.replaceAll('_', ' ')}
                      {' · '}
                      {item.target_actor
                        ? 'Named person'
                        : targetRole
                          ? roleLabel(targetRole)
                          : 'Destination unavailable'}
                    </p>
                  </div>
                  <Badge variant={item.state === 'requires_human' ? 'secondary' : 'outline'}>
                    {stateLabels[item.state]}
                  </Badge>
                </div>
                {item.target_actor ? (
                  <ActorIdentity
                    actor={item.target_actor}
                    agents={agents}
                    members={members}
                    daemons={daemons}
                  />
                ) : null}
                {item.findings.length ? (
                  <div>
                    <p className="text-xs font-medium">Findings</p>
                    <ul className="mt-1 list-disc space-y-1 pl-5 text-sm text-muted-foreground">
                      {item.findings.map((finding, index) => (
                        <li key={`${index}:${finding}`} className="break-words">
                          {finding}
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}
                <p className="text-sm text-muted-foreground">
                  <span className="font-medium text-foreground">Resolution condition:</span>{' '}
                  {item.resolution_condition}
                </p>
                {item.state === 'notified' && !item.work_unit_id ? (
                  <p className="text-xs text-muted-foreground">
                    A Handoff was recorded. No WorkUnit link was returned.
                  </p>
                ) : null}
                {item.work_unit_id ? (
                  <div className="flex min-w-0 items-center gap-1 text-xs">
                    <span>Work item created:</span>
                    <code className="min-w-0 break-all">{item.work_unit_id}</code>
                    <CopyReferenceButton value={item.work_unit_id} label="WorkUnit ID" />
                  </div>
                ) : null}
                {item.handoff_id ? (
                  <details className="text-xs text-muted-foreground">
                    <summary className="w-fit cursor-pointer">Handoff reference</summary>
                    <div className="mt-1 flex min-w-0 items-center gap-1">
                      <code className="min-w-0 break-all">{item.handoff_id}</code>
                      <CopyReferenceButton value={item.handoff_id} label="Handoff ID" />
                    </div>
                  </details>
                ) : null}
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs text-muted-foreground">Round {item.budget_round}</span>
                  {!isTerminal(item.state) ? (
                    <div className="flex flex-wrap gap-2">
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={resolveRequest.isPending}
                        onClick={() => void resolve(item, 'resolved')}
                      >
                        Mark resolved
                      </Button>
                      {item.state !== 'requires_human' ? (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={resolveRequest.isPending}
                          onClick={() => void resolve(item, 'requires_human')}
                        >
                          Needs human decision
                        </Button>
                      ) : null}
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={resolveRequest.isPending}
                        onClick={() => void resolve(item, 'cancelled')}
                      >
                        Cancel request
                      </Button>
                    </div>
                  ) : null}
                  <details className="text-xs text-muted-foreground">
                    <summary className="w-fit cursor-pointer">Request reference</summary>
                    <div className="mt-1 flex min-w-0 items-center gap-1">
                      <code className="min-w-0 break-all">{item.id}</code>
                      <CopyReferenceButton value={item.id} label="Quality request ID" />
                    </div>
                  </details>
                </div>
              </li>
            )
          })}
        </ul>
      ) : query.data ? (
        <p className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
          No quality requests have been recorded for this Task.
        </p>
      ) : null}

      <CollapsibleSection
        title="Create a quality request"
        badge={coordination.max_quality_requests - coordination.quality_request_count}
      >
        {actionError ? (
          <p role="alert" className="text-sm text-destructive">
            {actionError}
          </p>
        ) : null}
        <form
          onSubmit={(event) => void submit(event)}
          className="space-y-4 rounded-md bg-muted/20 p-3"
        >
          <div className="space-y-2">
            <Label htmlFor="quality-category">Route</Label>
            <Select
              id="quality-category"
              value={category}
              options={categoryOptions.map(({ value, label, target }) => ({
                value,
                label: `${label} → ${target}`,
              }))}
              onChange={(value) => {
                setCategory(value as TaskQualityCategory)
                setRoleId('')
                setActorKey('')
              }}
            />
          </div>
          {category === 'human_decision' ? (
            <div className="space-y-2">
              <Label htmlFor="quality-human">Person to notify</Label>
              {humanOptions.length ? (
                <Select
                  id="quality-human"
                  value={actorKey}
                  options={humanOptions}
                  onChange={setActorKey}
                  placeholder="Select a Task team member"
                />
              ) : (
                <p className="text-sm text-muted-foreground">
                  No active Human is present in this Task team. Add a named Human to the team before
                  routing this request.
                </p>
              )}
            </div>
          ) : (
            <div className="space-y-2">
              <Label htmlFor="quality-role">Destination role</Label>
              {roleOptions.length ? (
                <Select
                  id="quality-role"
                  value={roleId}
                  options={roleOptions}
                  onChange={setRoleId}
                  placeholder="Select an active role"
                />
              ) : (
                <p className="text-sm text-muted-foreground">
                  No active role matches this route. Add an eligible role and member in Advanced
                  work.
                </p>
              )}
            </div>
          )}
          <div className="space-y-2">
            <Label htmlFor="quality-summary">What needs attention?</Label>
            <Input
              id="quality-summary"
              value={summary}
              onChange={(event) => setSummary(event.target.value)}
              maxLength={8192}
              required
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="quality-findings">Findings</Label>
            <Textarea
              id="quality-findings"
              value={findings}
              onChange={(event) => setFindings(event.target.value)}
              placeholder="One finding per line"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="quality-resolution">How will this be considered resolved?</Label>
            <Textarea
              id="quality-resolution"
              value={resolutionCondition}
              onChange={(event) => setResolutionCondition(event.target.value)}
              required
            />
          </div>
          <details className="space-y-2 text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer">Link a reviewed result (optional)</summary>
            <Label htmlFor="quality-source-artifact">ReviewReport Artifact ID</Label>
            <Input
              id="quality-source-artifact"
              value={sourceArtifactId}
              onChange={(event) => setSourceArtifactId(event.target.value)}
              placeholder="Paste an exact ReviewReport reference"
            />
          </details>
          <Button type="submit" size="sm" disabled={!canSubmit || createRequest.isPending}>
            {createRequest.isPending ? 'Recording…' : 'Record quality request'}
          </Button>
          <p className="text-xs text-muted-foreground">
            The server applies category-to-destination rules and the Task’s request budget. A
            notification or Handoff does not mean work has started.
          </p>
        </form>
      </CollapsibleSection>
    </section>
  )
}
