import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { toast } from 'sonner'
import {
  useAgentAvailability,
  useAuthorizeTaskCoordination,
  useCreateTaskIntervention,
  useMaterializeTaskCoordinationPlan,
  useRequestTaskCoordinationTurn,
  useTaskCoordinationPlanQuery,
  useTaskCoordinationQuery,
  useTaskCoordinationRequestQuery,
} from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Textarea } from '@/components/ui/textarea'
import { ActorIdentity, resolveActorIdentity } from '@/features/task-roles/ActorIdentity'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  AgentResponse,
  DaemonResponse,
  ProjectMemberResponse,
  Task,
  TaskCoordinationMode,
  TaskCoordinationOverviewResponse,
  TaskCoordinationPlanResponse,
  TaskCoordinationRequestResponse,
  TaskInterventionKind,
  TaskCoordinationPlanDecision,
} from '@/types/generated'
import { TaskInterventionsPanel } from './task-interventions-panel'
import { TaskQualityRequestsPanel } from './task-quality-requests-panel'

type CoordinatorChoice = {
  role: TaskCoordinationOverviewResponse['team'][number]
  member: TaskCoordinationOverviewResponse['team'][number]['members'][number]
  value: string
  label: string
}

const modeOptions: Array<{
  value: Exclude<TaskCoordinationMode, 'manual'>
  title: string
  description: string
}> = [
  {
    value: 'proposal',
    title: 'Prepare a proposal',
    description:
      'Forge prepares a read-only plan for review. No WorkUnits are created until you approve and materialize it.',
  },
  {
    value: 'prepare_and_execute',
    title: 'Prepare and execute',
    description:
      'Forge may materialize a plan and admit its Task dispatch intent when it fits the saved limits. Plans outside those limits return for approval.',
  },
]

const requestStateLabels: Record<TaskCoordinationRequestResponse['state'], string> = {
  requested: 'Request recorded',
  running: 'Preparing a plan',
  plan_ready: 'Plan is ready',
  awaiting_approval: 'Waiting for approval',
  materialized: 'Work items materialized',
  failed: 'Preparation failed',
  cancelled: 'Request cancelled',
  uncertain: 'Provider outcome is uncertain',
}

const planStateLabels: Record<TaskCoordinationPlanResponse['state'], string> = {
  proposed: 'Proposal',
  approved: 'Approved',
  rejected: 'Rejected',
  superseded: 'Superseded',
  materialized: 'Materialized',
}

const dispatchStateLabels: Record<TaskCoordinationOverviewResponse['dispatch']['state'], string> = {
  idle: 'No dispatch intent',
  running: 'Dispatch intent is running',
  paused: 'Dispatch is paused',
  cancelled: 'Dispatch is cancelled',
}

function newIdempotencyKey() {
  return crypto.randomUUID()
}

function roleLabel(role: TaskCoordinationOverviewResponse['team'][number]) {
  return role.display_label?.trim() || role.role.replaceAll('_', ' ')
}

function readableCode(code: string) {
  const labels: Record<string, string> = {
    'coordination.authorization_revoked': 'Coordination authorization has been revoked.',
    'coordination.authorization_stale':
      'The saved Task team changed. Review and authorize it again.',
    'coordination.budget_exhausted': 'The Task has reached its coordination turn limit.',
    'coordination.coordinator_capability_missing':
      'The selected role no longer has orchestration capability.',
    'coordination.coordinator_missing':
      'Select an active Agent in a role with orchestration capability.',
    'coordination.coordinator_not_member':
      'The selected coordinator is no longer an active role member.',
    'coordination.not_authorized':
      'The current Task team or authorization does not allow this action.',
  }
  return labels[code] ?? code.replaceAll('.', ' ').replaceAll('_', ' ')
}

function shortDigest(value: string) {
  return value.length > 16 ? `${value.slice(0, 12)}…${value.slice(-4)}` : value
}

function getCoordinatorChoices(
  overview: TaskCoordinationOverviewResponse,
  agents: AgentResponse[],
  daemons: DaemonResponse[],
  members: ProjectMemberResponse[],
): CoordinatorChoice[] {
  return overview.team.flatMap((role) => {
    if (!role.capabilities.includes('orchestration')) return []
    return role.members.flatMap((member) => {
      if (
        member.status !== 'active' ||
        member.actor_ref.kind !== 'agent' ||
        member.harness?.toLowerCase() !== 'codex'
      ) {
        return []
      }
      const identity = resolveActorIdentity(member.actor_ref, agents, members, daemons)
      return [
        {
          role,
          member,
          value: `${role.id}:${member.actor_ref.kind}:${member.actor_ref.id}`,
          label: `${identity.name} · ${identity.context} · ${roleLabel(role)}`,
        },
      ]
    })
  })
}

function requestNextAction(
  overview: TaskCoordinationOverviewResponse,
  request: TaskCoordinationRequestResponse | null,
  plan: TaskCoordinationPlanResponse | null,
) {
  if (overview.revoked_at) return 'Coordination authorization is revoked.'
  if (overview.can_authorize) return 'Choose a coordinator and authorization mode.'
  if (request?.state === 'requested' || request?.state === 'running') {
    return 'Forge is preparing the requested plan.'
  }
  if (plan?.state === 'proposed') return 'Review the proposed work before creating WorkUnits.'
  if (plan?.state === 'approved' && !plan.materialization) {
    return 'The plan is approved; materialize it to create WorkUnits.'
  }
  if (plan?.materialization)
    return `Work items exist. ${dispatchStateLabels[overview.dispatch.state]}.`
  if (overview.can_request) return 'Describe the outcome and request a preparation turn.'
  if (overview.blockers.length) return readableCode(overview.blockers[0].code)
  return 'Continue using the saved Task team and current coordination state.'
}

export function TaskCoordinationPanel({
  task,
  agents,
  members,
  daemons,
  onOpenAdvancedWork,
}: {
  task: Task
  agents: AgentResponse[]
  members: ProjectMemberResponse[]
  daemons: DaemonResponse[]
  onOpenAdvancedWork: () => void
}) {
  const query = useTaskCoordinationQuery(task.id)
  const overview = query.data
  const planQuery = useTaskCoordinationPlanQuery(
    task.id,
    overview?.latest_plan?.artifact_id ?? null,
  )
  const overviewPlan = overview?.latest_plan ?? null
  const plan = planQuery.data ?? overviewPlan
  const requestId = plan?.request_id ?? overview?.latest_request?.id ?? null
  const requestQuery = useTaskCoordinationRequestQuery(task.id, requestId)
  const request = requestQuery.data ?? overview?.latest_request ?? null
  const authorize = useAuthorizeTaskCoordination()
  const requestTurn = useRequestTaskCoordinationTurn()
  const createIntervention = useCreateTaskIntervention()
  const materialize = useMaterializeTaskCoordinationPlan()
  const choices = useMemo(
    () => (overview ? getCoordinatorChoices(overview, agents, daemons, members) : []),
    [agents, daemons, members, overview],
  )
  const [mode, setMode] = useState<Exclude<TaskCoordinationMode, 'manual'>>('proposal')
  const [coordinatorKey, setCoordinatorKey] = useState('')
  const [objective, setObjective] = useState(task.title)
  const [constraints, setConstraints] = useState('')
  const [acceptanceCriteria, setAcceptanceCriteria] = useState('')
  const [context, setContext] = useState(task.description ?? '')
  const [decisionRationale, setDecisionRationale] = useState('')
  const [materializationOutcome, setMaterializationOutcome] = useState<{
    launch_intent_accepted: boolean
    launch_blockers: string[]
  } | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  useEffect(() => {
    setObjective(task.title)
    setContext(task.description ?? '')
  }, [task.description, task.id, task.title])

  useEffect(() => {
    if (!coordinatorKey || !choices.some((choice) => choice.value === coordinatorKey)) {
      setCoordinatorKey(choices[0]?.value ?? '')
    }
  }, [choices, coordinatorKey])

  const selectedCoordinator = choices.find((choice) => choice.value === coordinatorKey)
  const coordinatorAgentId =
    overview?.coordinator?.actor_ref.kind === 'agent'
      ? overview.coordinator.actor_ref.id
      : overview?.can_authorize && selectedCoordinator?.member.actor_ref.kind === 'agent'
        ? selectedCoordinator.member.actor_ref.id
        : null
  const coordinatorAvailability = useAgentAvailability(coordinatorAgentId)
  const supportedCoordinator = overview?.coordinator?.harness?.toLowerCase() === 'codex'
  const coordinatorAvailable = Boolean(
    supportedCoordinator && coordinatorAvailability.data?.available,
  )
  const plannedModeIsAuthorized = Boolean(
    overview && overview.mode !== 'manual' && !overview.revoked_at,
  )
  const nextAction = overview
    ? overview.can_request
      ? coordinatorAvailability.isLoading
        ? 'Checking the current coordinator Agent and host.'
        : coordinatorAvailability.isError
          ? 'The current coordinator availability could not be checked.'
          : !supportedCoordinator
            ? 'The selected coordinator harness is not supported for automatic preparation.'
            : !coordinatorAvailable
              ? `The coordinator is not available to accept a turn${coordinatorAvailability.data?.reason ? `: ${coordinatorAvailability.data.reason}` : '.'}`
              : requestNextAction(overview, request, plan)
      : requestNextAction(overview, request, plan)
    : ''

  async function authorizeCoordination() {
    if (!overview || !overview.can_authorize || !selectedCoordinator) return
    setActionError(null)
    try {
      await authorize.mutateAsync({
        taskId: task.id,
        body: {
          expected_task_version: overview.task_version,
          expected_coordination_version: overview.authorization_version,
          mode,
          coordinator_task_role_id: selectedCoordinator.role.id,
          coordinator_actor: selectedCoordinator.member.actor_ref,
          envelope: null,
          idempotency_key: newIdempotencyKey(),
        },
      })
      toast.success('Task coordination authorization saved')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Task coordination could not be authorized')
      setActionError(message)
      toast.error(message)
    }
  }

  async function requestPlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!overview?.can_request || !coordinatorAvailable || !objective.trim()) return
    setActionError(null)
    try {
      const response = await requestTurn.mutateAsync({
        taskId: task.id,
        body: {
          expected_task_version: overview.task_version,
          expected_coordination_version: overview.authorization_version,
          objective: objective.trim(),
          constraints: constraints.trim() || null,
          acceptance_criteria: acceptanceCriteria
            .split('\n')
            .map((criterion) => criterion.trim())
            .filter(Boolean),
          context: context.trim() || null,
          idempotency_key: newIdempotencyKey(),
        },
      })
      if (!response.accepted) {
        toast.message('The server did not accept the coordination request')
      } else {
        toast.success('Coordination request recorded')
      }
    } catch (error) {
      const message = getApiErrorMessage(error, 'Plan preparation could not be requested')
      setActionError(message)
      toast.error(message)
    }
  }

  async function decidePlan(decision: TaskCoordinationPlanDecision) {
    if (!overview || !plan || plan.state !== 'proposed') return
    setActionError(null)
    const kind: TaskInterventionKind = 'decision'
    try {
      const response = await createIntervention.mutateAsync({
        taskId: task.id,
        body: {
          kind,
          expected_task_version: overview.task_version,
          expected_coordination_version: overview.authorization_version,
          expected_lifecycle_version: null,
          instruction: null,
          plan_artifact_id: plan.artifact_id,
          decision,
          rationale: decisionRationale.trim() || null,
          reason: null,
          brief: null,
          idempotency_key: newIdempotencyKey(),
        },
      })
      if (response.state === 'rejected') {
        throw new Error(
          response.failure_message ?? response.failure_code ?? 'Plan decision was rejected',
        )
      }
      toast.success(decision === 'approve' ? 'Plan approval recorded' : 'Plan rejection recorded')
      setDecisionRationale('')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Plan decision could not be recorded')
      setActionError(message)
      toast.error(message)
    }
  }

  async function materializePlan() {
    if (!overview || !plan || !request || plan.state !== 'approved' || plan.materialization) return
    setActionError(null)
    try {
      const result = await materialize.mutateAsync({
        taskId: task.id,
        artifactId: plan.artifact_id,
        body: {
          expected_task_version: overview.task_version,
          expected_coordination_version: overview.authorization_version,
          expected_plan_digest: plan.digest,
          expected_plan_version: plan.version,
          expected_request_version: request.version,
          idempotency_key: newIdempotencyKey(),
        },
      })
      setMaterializationOutcome({
        launch_intent_accepted: result.launch_intent_accepted,
        launch_blockers: result.launch_blockers,
      })
      toast.success('WorkUnits were materialized from the approved plan')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Approved plan could not be materialized')
      setActionError(message)
      toast.error(message)
    }
  }

  if (query.isLoading) {
    return (
      <section className="rounded-lg border p-4 text-sm text-muted-foreground">
        Loading Task coordination…
      </section>
    )
  }

  if (query.isError || !overview) {
    return (
      <section className="rounded-lg border p-4" aria-labelledby="task-coordination-title">
        <h2 id="task-coordination-title" className="font-semibold">
          Plan and next step
        </h2>
        <div
          role="alert"
          className="mt-3 rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          The Task coordination state could not be loaded. No new coordination action is available
          until Forge returns its current state.
          <Button size="sm" variant="outline" className="ml-2" onClick={() => void query.refetch()}>
            Retry
          </Button>
        </div>
      </section>
    )
  }

  return (
    <section
      className="min-w-0 space-y-4 rounded-lg border border-primary/30 bg-card p-4 sm:p-5"
      aria-labelledby="task-coordination-title"
    >
      <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id="task-coordination-title" className="text-lg font-semibold">
              Plan and next step
            </h2>
            <Badge variant="outline">
              {overview.mode === 'manual'
                ? 'Manual planning'
                : (modeOptions.find((option) => option.value === overview.mode)?.title ??
                  overview.mode)}
            </Badge>
            {overview.revoked_at ? (
              <Badge variant="destructive">Authorization revoked</Badge>
            ) : null}
          </div>
          <p className="mt-1 text-sm text-muted-foreground">{nextAction}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span>
            {overview.turn_count} / {overview.max_turns} preparation turns
          </span>
          <span>·</span>
          <span>{dispatchStateLabels[overview.dispatch.state]}</span>
        </div>
      </header>

      {overview.coordinator ? (
        <div className="rounded-md border p-3">
          <p className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Selected coordinator
          </p>
          <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
            <ActorIdentity
              actor={overview.coordinator.actor_ref}
              agents={agents}
              members={members}
              daemons={daemons}
            />
            <span className="text-sm text-muted-foreground">
              {overview.coordinator.role.replaceAll('_', ' ')} · orchestration
            </span>
          </div>
        </div>
      ) : null}

      {overview.blockers.length ? (
        <div
          className="space-y-2 rounded-md border border-amber-400/50 bg-amber-50 p-3 text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100"
          aria-labelledby="coordination-blockers-title"
        >
          <h3 id="coordination-blockers-title" className="text-sm font-semibold">
            Coordination needs attention
          </h3>
          <ul className="space-y-1 text-sm">
            {overview.blockers.map((blocker, index) => (
              <li key={`${blocker.code}:${index}`} className="break-words">
                {readableCode(blocker.code)}
                <details className="mt-1 text-xs text-muted-foreground">
                  <summary className="w-fit cursor-pointer">Technical details</summary>
                  <p className="mt-1 break-all">{blocker.message}</p>
                  {blocker.task_role_id ? (
                    <p>
                      TaskRole: <code>{blocker.task_role_id}</code>
                    </p>
                  ) : null}
                </details>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {actionError ? (
        <p
          role="alert"
          className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          {actionError}
        </p>
      ) : null}
      {planQuery.isError ? (
        <p
          role="alert"
          className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          The exact plan could not be retrieved. Review actions are unavailable until the plan can
          be refreshed.{' '}
          <Button size="sm" variant="outline" onClick={() => void planQuery.refetch()}>
            Retry
          </Button>
        </p>
      ) : null}
      {requestQuery.isError ? (
        <p
          role="alert"
          className="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          The exact coordination request could not be retrieved.{' '}
          <Button size="sm" variant="outline" onClick={() => void requestQuery.refetch()}>
            Retry
          </Button>
        </p>
      ) : null}

      {overview.can_authorize ? (
        <div className="space-y-4 rounded-md border p-3">
          <div>
            <h3 className="font-medium">Authorize a Task coordinator</h3>
            <p className="mt-1 text-sm text-muted-foreground">
              Choose a current Agent member of a role that has the orchestration capability. The
              current turn runtime supports the Codex CLI path; Forge checks the selected Agent and
              host again before admission.
            </p>
          </div>
          {choices.length ? (
            <div className="space-y-2">
              <Label htmlFor="coordination-coordinator">
                Coordinator (name · harness · host · role)
              </Label>
              <select
                id="coordination-coordinator"
                value={coordinatorKey}
                onChange={(event) => setCoordinatorKey(event.target.value)}
                className="flex h-10 w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {choices.map((choice) => (
                  <option key={choice.value} value={choice.value}>
                    {choice.label}
                  </option>
                ))}
              </select>
              {selectedCoordinator ? (
                <div
                  role="status"
                  className="flex flex-wrap items-center gap-2 rounded-md bg-muted/30 px-3 py-2 text-xs text-muted-foreground"
                >
                  <span>
                    {coordinatorAvailability.isLoading
                      ? 'Checking the selected Agent and host…'
                      : coordinatorAvailability.isError
                        ? 'Current availability could not be checked.'
                        : coordinatorAvailability.data?.available
                          ? `Available on ${daemons.find((daemon) => daemon.id === coordinatorAvailability.data.resolved_daemon_id)?.hostname ?? 'the resolved host'}.`
                          : `Not currently available (${coordinatorAvailability.data?.effective_status ?? 'status unavailable'}). ${coordinatorAvailability.data?.reason ?? ''}`}
                  </span>
                  {coordinatorAvailability.isError ? (
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => void coordinatorAvailability.refetch()}
                    >
                      Retry status
                    </Button>
                  ) : null}
                </div>
              ) : null}
            </div>
          ) : (
            <div className="rounded-md border border-dashed p-3">
              <p className="text-sm font-medium">
                No eligible coordinator is available in this Task team.
              </p>
              <p className="mt-1 text-sm text-muted-foreground">
                Add an active Codex CLI Agent to a Task role with orchestration capability. Existing
                Task roles and memberships can be edited in Advanced work.
              </p>
              <Button size="sm" variant="outline" className="mt-3" onClick={onOpenAdvancedWork}>
                Open Advanced work
              </Button>
            </div>
          )}
          <fieldset className="space-y-3" disabled={!choices.length}>
            <legend className="text-sm font-medium">How much autonomy do you authorize?</legend>
            {modeOptions.map((option) => (
              <label
                key={option.value}
                className={`flex min-w-0 cursor-pointer gap-3 rounded-md border p-3 ${mode === option.value ? 'border-primary bg-primary/5' : ''}`}
              >
                <input
                  type="radio"
                  name="coordination-mode"
                  value={option.value}
                  checked={mode === option.value}
                  onChange={() => setMode(option.value)}
                  className="mt-1 accent-primary"
                />
                <span className="min-w-0">
                  <span className="block text-sm font-medium">{option.title}</span>
                  <span className="mt-1 block text-sm text-muted-foreground">
                    {option.description}
                  </span>
                </span>
              </label>
            ))}
          </fieldset>
          <details className="text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer">
              Current coordination limits and team snapshot
            </summary>
            <div className="mt-2 space-y-2">
              <p>
                Team digest: <code className="break-all">{overview.team_digest}</code>
              </p>
              <p>
                Limits digest: <code className="break-all">{overview.envelope_digest}</code>
              </p>
              <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words rounded bg-muted p-2">
                {JSON.stringify(overview.envelope, null, 2)}
              </pre>
              <p>
                Authorization version {overview.authorization_version} · Task version{' '}
                {overview.task_version}
              </p>
            </div>
          </details>
          <Button
            size="sm"
            onClick={() => void authorizeCoordination()}
            disabled={!selectedCoordinator || authorize.isPending}
          >
            {authorize.isPending ? 'Saving authorization…' : 'Save authorization'}
          </Button>
        </div>
      ) : null}

      {plannedModeIsAuthorized && overview.can_request && coordinatorAvailable ? (
        <form
          onSubmit={(event) => void requestPlan(event)}
          className="space-y-4 rounded-md border p-3"
        >
          <div>
            <h3 className="font-medium">Prepare from the Task objective</h3>
            <p className="mt-1 text-sm text-muted-foreground">
              The request uses the saved authorization and current Task team. It does not require
              manual WorkUnit setup.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="coordination-objective">Outcome</Label>
            <Textarea
              id="coordination-objective"
              value={objective}
              onChange={(event) => setObjective(event.target.value)}
              required
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="coordination-constraints">Constraints and boundaries</Label>
            <Textarea
              id="coordination-constraints"
              value={constraints}
              onChange={(event) => setConstraints(event.target.value)}
              placeholder="Scope, read/write boundaries, and actions that remain out of scope…"
            />
            <p className="text-xs text-muted-foreground">
              This context does not grant new permissions. Forge checks the Task team’s current
              authority and the selected host before acting.
            </p>
          </div>
          <div className="space-y-2">
            <Label htmlFor="coordination-acceptance">Acceptance criteria</Label>
            <Textarea
              id="coordination-acceptance"
              value={acceptanceCriteria}
              onChange={(event) => setAcceptanceCriteria(event.target.value)}
              placeholder="One criterion per line"
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="coordination-context">Additional context</Label>
            <Textarea
              id="coordination-context"
              value={context}
              onChange={(event) => setContext(event.target.value)}
            />
          </div>
          <Button size="sm" type="submit" disabled={!objective.trim() || requestTurn.isPending}>
            {requestTurn.isPending
              ? 'Recording request…'
              : modeOptions.find((option) => option.value === overview.mode)?.value === 'proposal'
                ? 'Prepare proposal'
                : 'Prepare and execute'}
          </Button>
        </form>
      ) : null}

      {plannedModeIsAuthorized && overview.can_request && !coordinatorAvailable ? (
        <div
          role={coordinatorAvailability.isError ? 'alert' : 'status'}
          className="rounded-md border border-dashed p-3 text-sm text-muted-foreground"
        >
          {coordinatorAvailability.isLoading
            ? 'Checking whether the current coordinator can accept a turn…'
            : coordinatorAvailability.isError
              ? 'Forge availability could not be checked. No coordination turn has been submitted.'
              : !supportedCoordinator
                ? 'The current coordinator is not a Codex CLI Agent. Reauthorize with a supported coordinator before preparing a plan.'
                : `No preparation request was sent because the coordinator is not currently available${coordinatorAvailability.data?.reason ? `: ${coordinatorAvailability.data.reason}` : '.'}`}
          {coordinatorAvailability.isError ? (
            <Button
              size="sm"
              variant="outline"
              className="ml-2"
              onClick={() => void coordinatorAvailability.refetch()}
            >
              Retry status
            </Button>
          ) : null}
        </div>
      ) : null}

      {!plannedModeIsAuthorized && !overview.can_authorize ? (
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-dashed p-3">
          <div className="min-w-0">
            <p className="text-sm font-medium">Manual work remains available</p>
            <p className="mt-1 text-sm text-muted-foreground">
              Edit roles, memberships, and WorkUnits in the advanced path. Coordination actions
              remain unavailable unless the server allows them.
            </p>
          </div>
          <Button size="sm" variant="outline" onClick={onOpenAdvancedWork}>
            Open Advanced work
          </Button>
        </div>
      ) : null}

      {request ? (
        <article
          className="min-w-0 rounded-md border bg-muted/10 p-3"
          aria-labelledby="coordination-request-title"
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 id="coordination-request-title" className="font-medium">
              Latest preparation request
            </h3>
            <Badge
              variant={
                request.state === 'failed' || request.state === 'uncertain'
                  ? 'destructive'
                  : 'outline'
              }
            >
              {requestStateLabels[request.state]}
            </Badge>
          </div>
          <p className="mt-2 break-words text-sm">{request.brief.objective}</p>
          {request.brief.constraints ? (
            <p className="mt-1 whitespace-pre-wrap break-words text-sm text-muted-foreground">
              Constraints: {request.brief.constraints}
            </p>
          ) : null}
          {request.brief.acceptance_criteria.length ? (
            <ul className="mt-2 list-disc space-y-1 pl-5 text-sm text-muted-foreground">
              {request.brief.acceptance_criteria.map((criterion, index) => (
                <li key={`${index}:${criterion}`} className="break-words">
                  {criterion}
                </li>
              ))}
            </ul>
          ) : null}
          {request.state === 'uncertain' ? (
            <p className="mt-2 text-sm text-amber-700 dark:text-amber-300">
              The provider outcome is uncertain. Forge will not automatically repeat the external
              start.
            </p>
          ) : null}
          {request.failure_message ? (
            <p role="alert" className="mt-2 break-words text-sm text-destructive">
              {request.failure_message}
            </p>
          ) : null}
          {request.failure_code ? (
            <p className="mt-1 text-xs text-muted-foreground">Failure: {request.failure_code}</p>
          ) : null}
          <details className="mt-2 text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer">Request references</summary>
            <div className="mt-1 space-y-1">
              <p>
                Objective digest: <code className="break-all">{request.objective_digest}</code>
              </p>
              <p>
                Request digest: <code className="break-all">{request.request_digest}</code>
              </p>
              <p>
                Authorization version {request.authorization_version} · request version{' '}
                {request.version}
              </p>
              {request.wake_id ? (
                <p>
                  Wake: <code className="break-all">{request.wake_id}</code>
                </p>
              ) : null}
              {request.execution_id ? (
                <p>
                  Execution: <code className="break-all">{request.execution_id}</code>
                </p>
              ) : null}
            </div>
          </details>
        </article>
      ) : null}

      {plan ? (
        <article
          className="min-w-0 space-y-4 rounded-md border p-3"
          aria-labelledby="coordination-plan-title"
        >
          <div className="flex flex-wrap items-start justify-between gap-2">
            <div className="min-w-0">
              <h3 id="coordination-plan-title" className="font-medium">
                Prepared work plan
              </h3>
              <p className="mt-1 break-all text-xs text-muted-foreground">
                Digest {shortDigest(plan.digest)} · {plan.work_units.length} planned work item
                {plan.work_units.length === 1 ? '' : 's'}
              </p>
            </div>
            <Badge
              variant={
                plan.state === 'rejected' || plan.state === 'superseded' ? 'secondary' : 'outline'
              }
            >
              {planStateLabels[plan.state]}
            </Badge>
          </div>
          {plan.work_units.length ? (
            <ol className="grid min-w-0 gap-3 lg:grid-cols-2">
              {plan.work_units.map((unit) => {
                const role = overview.team.find((item) => item.role === unit.role)
                return (
                  <li key={unit.key} className="min-w-0 space-y-2 rounded-md border p-3">
                    <div className="flex flex-wrap items-start justify-between gap-2">
                      <div className="min-w-0">
                        <h4 className="break-words text-sm font-semibold">{unit.title}</h4>
                        <p className="mt-1 text-xs text-muted-foreground">
                          {role ? roleLabel(role) : unit.role.replaceAll('_', ' ')}
                        </p>
                      </div>
                      <Badge variant="outline">
                        {unit.requires_integration
                          ? 'Integration required'
                          : 'No integration required'}
                      </Badge>
                    </div>
                    <p className="whitespace-pre-wrap break-words text-sm text-muted-foreground">
                      {unit.scope}
                    </p>
                    {unit.assigned_actor ? (
                      <ActorIdentity
                        actor={unit.assigned_actor}
                        agents={agents}
                        members={members}
                        daemons={daemons}
                      />
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        No Actor is assigned in this plan.
                      </p>
                    )}
                    {unit.acceptance.length ? (
                      <div>
                        <p className="text-xs font-medium">Acceptance</p>
                        <ul className="mt-1 list-disc space-y-1 pl-5 text-xs text-muted-foreground">
                          {unit.acceptance.map((criterion, index) => (
                            <li key={`${index}:${criterion}`} className="break-words">
                              {criterion}
                            </li>
                          ))}
                        </ul>
                      </div>
                    ) : null}
                    <details className="text-xs text-muted-foreground">
                      <summary className="w-fit cursor-pointer">
                        Dependencies and technical context
                      </summary>
                      <div className="mt-2 space-y-1 break-words">
                        <p>
                          Plan key: <code>{unit.key}</code>
                        </p>
                        {unit.parent_key ? (
                          <p>
                            Parent plan key: <code>{unit.parent_key}</code>
                          </p>
                        ) : null}
                        <p>
                          Depends on:{' '}
                          {unit.depends_on.length ? unit.depends_on.join(', ') : 'No prerequisites'}
                        </p>
                        {unit.review_subject_key ? (
                          <p>
                            Review subject key: <code>{unit.review_subject_key}</code>
                          </p>
                        ) : null}
                        {unit.execution_instructions ? (
                          <p className="whitespace-pre-wrap">
                            Instructions: {unit.execution_instructions}
                          </p>
                        ) : null}
                        {unit.artifact_inputs.map((artifactId) => (
                          <div key={artifactId} className="flex min-w-0 items-center gap-1">
                            <span>Artifact:</span>
                            <code className="min-w-0 break-all">{artifactId}</code>
                            <CopyReferenceButton value={artifactId} label="Artifact ID" />
                          </div>
                        ))}
                      </div>
                    </details>
                  </li>
                )
              })}
            </ol>
          ) : (
            <p className="text-sm text-muted-foreground">
              The server returned a plan with no WorkUnits.
            </p>
          )}

          {plan.state === 'proposed' ? (
            <div className="space-y-3 rounded-md bg-muted/20 p-3">
              <p className="text-sm">
                Review this exact digest. Approval records a durable Task intervention. Rejection
                keeps the proposal in its recorded history.
              </p>
              <div className="space-y-2">
                <Label htmlFor="plan-decision-rationale">Decision note (optional)</Label>
                <Input
                  id="plan-decision-rationale"
                  value={decisionRationale}
                  onChange={(event) => setDecisionRationale(event.target.value)}
                />
              </div>
              <div className="flex flex-wrap gap-2">
                <Button
                  size="sm"
                  disabled={createIntervention.isPending || planQuery.isError}
                  onClick={() => void decidePlan('approve')}
                >
                  Approve plan
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={createIntervention.isPending || planQuery.isError}
                  onClick={() => void decidePlan('reject')}
                >
                  Reject proposal
                </Button>
              </div>
            </div>
          ) : null}

          {plan.materialization ? (
            <div className="space-y-2 rounded-md border border-emerald-700/30 bg-emerald-50 p-3 text-emerald-950 dark:border-emerald-400/30 dark:bg-emerald-950/20 dark:text-emerald-100">
              <p className="text-sm font-medium">Materialization receipt returned by Forge</p>
              <p className="text-sm">
                {plan.materialization.work_unit_ids.length} WorkUnit ID
                {plan.materialization.work_unit_ids.length === 1 ? '' : 's'} recorded. Current
                dispatch: {dispatchStateLabels[overview.dispatch.state]}.
              </p>
              <details className="text-xs">
                <summary className="w-fit cursor-pointer">Receipt and WorkUnit references</summary>
                <div className="mt-2 space-y-2">
                  <p>
                    Receipt digest:{' '}
                    <code className="break-all">{plan.materialization.receipt_digest}</code>
                  </p>
                  <ul className="space-y-1">
                    {plan.materialization.work_unit_ids.map((id) => (
                      <li key={id} className="flex min-w-0 items-center gap-1">
                        <code className="min-w-0 break-all">{id}</code>
                        <CopyReferenceButton value={id} label="WorkUnit ID" />
                      </li>
                    ))}
                  </ul>
                </div>
              </details>
            </div>
          ) : plan.state === 'proposed' || plan.state === 'approved' ? (
            <p className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
              No WorkUnit has been created from this plan yet.
            </p>
          ) : null}

          {materializationOutcome ? (
            <div role="status" className="rounded-md border p-3 text-sm">
              {materializationOutcome.launch_intent_accepted ? (
                <p>
                  Forge accepted the Task dispatch intent. This does not claim that a provider has
                  started work.
                </p>
              ) : (
                <p>
                  WorkUnits were materialized; Forge did not accept a Task dispatch intent in this
                  response.
                </p>
              )}
              {materializationOutcome.launch_blockers.length ? (
                <ul className="mt-2 list-disc pl-5 text-muted-foreground">
                  {materializationOutcome.launch_blockers.map((blocker) => (
                    <li key={blocker} className="break-words">
                      {blocker}
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          ) : null}

          {plan.state === 'approved' && !plan.materialization ? (
            <div className="space-y-2 rounded-md border p-3">
              <p className="text-sm">This approved plan has no materialization receipt yet.</p>
              <Button
                size="sm"
                disabled={
                  !request || materialize.isPending || planQuery.isError || requestQuery.isError
                }
                onClick={() => void materializePlan()}
              >
                {materialize.isPending
                  ? 'Creating WorkUnits…'
                  : 'Create WorkUnits from approved plan'}
              </Button>
              {!request ? (
                <p className="text-xs text-muted-foreground">
                  The exact coordination request version is not available, so materialization is
                  disabled.
                </p>
              ) : null}
            </div>
          ) : null}

          <details className="text-xs text-muted-foreground">
            <summary className="w-fit cursor-pointer">Plan integrity references</summary>
            <div className="mt-2 space-y-1 break-all">
              <p>
                Plan Artifact: <code>{plan.artifact_id}</code>
              </p>
              <p>
                Plan digest: <code>{plan.digest}</code>
              </p>
              <p>
                Team digest: <code>{plan.team_digest}</code>
              </p>
              <p>
                Envelope digest: <code>{plan.envelope_digest}</code>
              </p>
              <p>
                Objective digest: <code>{plan.objective_digest}</code>
              </p>
              <p>
                Plan version {plan.version} · Task version {plan.task_version} · authorization
                version {plan.authorization_version}
              </p>
              <div className="flex flex-wrap gap-2">
                <CopyReferenceButton value={plan.artifact_id} label="Plan Artifact ID" />
                <CopyReferenceButton value={plan.digest} label="Plan digest" />
              </div>
            </div>
          </details>
        </article>
      ) : null}

      <TaskQualityRequestsPanel
        task={task}
        coordination={overview}
        agents={agents}
        members={members}
        daemons={daemons}
      />
      <TaskInterventionsPanel task={task} coordination={overview} />
    </section>
  )
}
