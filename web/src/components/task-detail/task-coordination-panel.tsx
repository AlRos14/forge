import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { Link } from '@tanstack/react-router'
import { toast } from 'sonner'
import { useAuthStore } from '@/stores/auth'
import {
  useAgentAvailability,
  useAuthorizeTaskCoordination,
  useCreateTaskIntervention,
  useMaterializeTaskCoordinationPlan,
  useRequestTaskCoordinationTurn,
  useTaskCoordinationPlanQuery,
  useTaskCoordinationQuery,
  useTaskCoordinationRequestQuery,
  useTaskReworkAutomationQuery,
  useTaskWorkUnitsQuery,
} from '@/api/hooks'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
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
import {
  canEditTaskCoordinationAuthorization,
  coordinationRoleLabel,
  deriveCoordinationNextAction,
  dispatchStateLabel,
  isProjectBatchScopedCoordination,
  isTurnBudgetExhausted,
  readableCoordinationCode,
} from './task-coordination-status'
import {
  deriveReviewFindingsProjection,
  isReviewFindingsRequest,
} from './task-review-triage'
import { TaskInterventionsPanel } from './task-interventions-panel'
import { TaskAutonomyLimitsPanel } from './task-autonomy-limits'
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
  triaged: 'Findings triaged',
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

function newIdempotencyKey() {
  return crypto.randomUUID()
}

function roleLabel(role: TaskCoordinationOverviewResponse['team'][number]) {
  return coordinationRoleLabel(role)
}

const nextActionToneClass: Record<ReturnType<typeof deriveCoordinationNextAction>['tone'], string> =
  {
    action: 'border-primary/40 bg-primary/5',
    wait: 'border-border-subtle bg-muted/20',
    blocked:
      'border-amber-400/60 bg-amber-50 text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100',
    done: 'border-emerald-700/30 bg-emerald-50 text-emerald-950 dark:border-emerald-400/30 dark:bg-emerald-950/20 dark:text-emerald-100',
    info: 'border-border-subtle bg-muted/20',
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
        !['codex', 'claude_code'].includes(member.harness?.toLowerCase() ?? '')
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
  const user = useAuthStore((state) => state.user)
  const canManageLimits = members.some(
    (member) => member.user_id === user?.id && ['owner', 'admin'].includes(member.role),
  )
  const query = useTaskCoordinationQuery(task.id)
  const overview = query.data
  const latestRequestIsReviewFindings = isReviewFindingsRequest(overview?.latest_request)
  const planQuery = useTaskCoordinationPlanQuery(
    task.id,
    overview?.latest_plan?.artifact_id ?? null,
  )
  const overviewPlan = overview?.latest_plan ?? null
  const plan = planQuery.data ?? overviewPlan
  const requestId = latestRequestIsReviewFindings
    ? (overview?.latest_request?.id ?? null)
    : (plan?.request_id ?? overview?.latest_request?.id ?? null)
  const requestQuery = useTaskCoordinationRequestQuery(task.id, requestId)
  const request = requestQuery.data ?? overview?.latest_request ?? null
  const workUnitsQuery = useTaskWorkUnitsQuery(task.id)
  const reworkAutomationQuery = useTaskReworkAutomationQuery(task.id)
  const automationIntentsState = reworkAutomationQuery.isError
    ? 'error'
    : reworkAutomationQuery.data === undefined
      ? 'loading'
      : 'loaded'
  const workUnitsState = workUnitsQuery.isError
    ? 'error'
    : workUnitsQuery.data === undefined
      ? 'loading'
      : 'loaded'
  const authorize = useAuthorizeTaskCoordination()
  const requestTurn = useRequestTaskCoordinationTurn()
  const createIntervention = useCreateTaskIntervention()
  const materialize = useMaterializeTaskCoordinationPlan()
  const choices = useMemo(
    () => (overview ? getCoordinatorChoices(overview, agents, daemons, members) : []),
    [agents, daemons, members, overview],
  )
  const currentCoordinatorValue = overview?.coordinator
    ? `${overview.coordinator.task_role_id}:${overview.coordinator.actor_ref.kind}:${overview.coordinator.actor_ref.id}`
    : ''
  const [mode, setMode] = useState<Exclude<TaskCoordinationMode, 'manual'>>('prepare_and_execute')
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
  const [revokeOpen, setRevokeOpen] = useState(false)
  const [revokeReason, setRevokeReason] = useState('')

  useEffect(() => {
    setObjective(task.title)
    setContext(task.description ?? '')
  }, [task.description, task.id, task.title])

  useEffect(() => {
    if (!choices.length) {
      if (coordinatorKey) setCoordinatorKey('')
      return
    }
    if (!coordinatorKey || !choices.some((choice) => choice.value === coordinatorKey)) {
      const current = choices.find((choice) => choice.value === currentCoordinatorValue)
      setCoordinatorKey(current?.value ?? choices[0]?.value ?? '')
    }
  }, [choices, coordinatorKey, currentCoordinatorValue])

  const authorizedMode = overview?.mode
  useEffect(() => {
    if (authorizedMode && authorizedMode !== 'manual') setMode(authorizedMode)
  }, [authorizedMode])

  const selectedCoordinator = choices.find((choice) => choice.value === coordinatorKey)
  const displayedCoordinatorAgent =
    overview?.can_authorize && selectedCoordinator
      ? selectedCoordinator.member
      : (overview?.coordinator ?? null)
  const coordinatorAgentId =
    displayedCoordinatorAgent?.actor_ref.kind === 'agent'
      ? displayedCoordinatorAgent.actor_ref.id
      : null
  const coordinatorAvailability = useAgentAvailability(coordinatorAgentId)
  const supportedCoordinator = ['codex', 'claude_code'].includes(
    displayedCoordinatorAgent?.harness?.toLowerCase() ?? '',
  )
  const coordinatorAvailable = Boolean(
    supportedCoordinator && coordinatorAvailability.data?.available,
  )
  const plannedModeIsAuthorized = Boolean(
    overview && overview.mode !== 'manual' && !overview.revoked_at,
  )
  const turnBudgetExhausted = overview ? isTurnBudgetExhausted(overview) : false
  const batchScopedCoordination = overview ? isProjectBatchScopedCoordination(overview) : false
  const canEditCoordinationAuthorization = overview
    ? canEditTaskCoordinationAuthorization(overview)
    : false
  const batchOwnsActiveAuthorization = Boolean(
    overview?.project_plan_batch_scope &&
    ['running', 'paused'].includes(overview.project_plan_batch_scope.batch_state),
  )
  const coordinatorRole = overview?.team.find(
    (role) => role.id === overview.coordinator?.task_role_id,
  )
  const derivedNextAction = overview
    ? deriveCoordinationNextAction(overview, request, plan, {
        automationIntents: reworkAutomationQuery.data?.items ?? [],
        automationIntentsState,
        workUnits: workUnitsQuery.data ?? [],
        workUnitsState,
      })
    : null
  const reviewFindingsProjection = overview
    ? deriveReviewFindingsProjection({
        overview,
        request,
        automationIntents: reworkAutomationQuery.data?.items ?? [],
        automationIntentsState,
        workUnits: workUnitsQuery.data ?? [],
        workUnitsState,
      })
    : null
  const baseNextAction =
    overview?.mode === 'manual' &&
    canEditCoordinationAuthorization &&
    !batchScopedCoordination &&
    selectedCoordinator
      ? {
          kind: 'request' as const,
          tone: 'action' as const,
          owner: 'You',
          summary: 'Ready to start with the selected coordinator.',
          detail:
            'Start authorizes the selected mode for this Task. The coordinator prepares the WorkUnits and the team can proceed within the saved boundaries.',
        }
      : derivedNextAction
  const nextAction = baseNextAction
    ? baseNextAction.kind === 'request' && coordinatorAvailability.isLoading
      ? {
          ...baseNextAction,
          summary: 'Checking the current coordinator Agent and host.',
          detail:
            'Forge checks availability again before admitting a read-only turn. No request has been sent.',
          tone: 'wait' as const,
          kind: 'wait' as const,
        }
      : baseNextAction.kind === 'request' && coordinatorAvailability.isError
        ? {
            ...baseNextAction,
            summary: 'Coordinator availability could not be checked.',
            detail:
              'No preparation request has been sent. Retry the status check before requesting a turn.',
            tone: 'blocked' as const,
            kind: 'blocked' as const,
          }
        : baseNextAction.kind === 'request' && !supportedCoordinator
          ? {
              ...baseNextAction,
              summary: 'The selected coordinator harness cannot accept an automatic turn.',
              detail:
                'The current turn runtime requires a Codex CLI Agent with the verified read-only path. Reauthorize with a supported Agent or continue through Advanced work.',
              tone: 'blocked' as const,
              kind: 'blocked' as const,
            }
          : baseNextAction.kind === 'request' && !coordinatorAvailable
            ? {
                ...baseNextAction,
                summary: 'The coordinator is not currently available.',
                detail: `No preparation request has been sent${coordinatorAvailability.data?.reason ? `: ${coordinatorAvailability.data.reason}` : '.'} Reauthorize with an available Agent or retry the status check.`,
                tone: 'blocked' as const,
                kind: 'blocked' as const,
              }
            : baseNextAction
    : null

  async function authorizeCoordination() {
    if (!overview || !canEditCoordinationAuthorization || !selectedCoordinator) return
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

  async function requestPlan(event?: FormEvent<HTMLFormElement>) {
    event?.preventDefault()
    if (
      !overview ||
      batchScopedCoordination ||
      (!overview.can_request && !overview.can_authorize) ||
      turnBudgetExhausted ||
      !coordinatorAvailable ||
      !selectedCoordinator ||
      !objective.trim()
    )
      return
    setActionError(null)
    try {
      const authorization =
        !plannedModeIsAuthorized ||
        overview.blockers.some((blocker) => blocker.code === 'coordination.authorization_stale') ||
        overview.mode !== mode ||
        selectedCoordinator.value !== currentCoordinatorValue
          ? await authorize.mutateAsync({
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
          : overview
      const response = await requestTurn.mutateAsync({
        taskId: task.id,
        body: {
          expected_task_version: authorization.task_version,
          expected_coordination_version: authorization.authorization_version,
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
    if (!overview || !plan || !request || !['approved', 'materialized'].includes(plan.state)) return
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
      toast.success(
        result.replayed
          ? 'Work preparation retried from the existing plan receipt'
          : 'WorkUnits were materialized from the approved plan',
      )
    } catch (error) {
      const message = getApiErrorMessage(error, 'Approved plan could not be materialized')
      setActionError(message)
      toast.error(message)
    }
  }

  async function resumePausedTask() {
    if (!overview || overview.dispatch.state !== 'paused') return
    setActionError(null)
    try {
      const response = await createIntervention.mutateAsync({
        taskId: task.id,
        body: {
          kind: 'resume',
          expected_task_version: overview.task_version,
          expected_coordination_version: overview.authorization_version,
          expected_lifecycle_version: overview.lifecycle.version,
          instruction: null,
          plan_artifact_id: null,
          decision: null,
          rationale: null,
          reason: 'Resume requested from the guided Task flow',
          brief: null,
          idempotency_key: newIdempotencyKey(),
        },
      })
      if (response.state === 'rejected') {
        throw new Error(
          response.failure_message ?? response.failure_code ?? 'Task resume was rejected',
        )
      }
      toast.success('Task dispatch resumed. Existing work was not restarted.')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Task could not be resumed')
      setActionError(message)
      toast.error(message)
    }
  }

  async function revokeAuthorization() {
    if (!overview || overview.mode === 'manual' || overview.revoked_at || !overview.can_authorize)
      return
    setActionError(null)
    try {
      const response = await createIntervention.mutateAsync({
        taskId: task.id,
        body: {
          kind: 'revoke_authorization',
          expected_task_version: overview.task_version,
          expected_coordination_version: overview.authorization_version,
          expected_lifecycle_version: null,
          instruction: null,
          plan_artifact_id: null,
          decision: null,
          rationale: null,
          reason: revokeReason.trim() || null,
          brief: null,
          idempotency_key: newIdempotencyKey(),
        },
      })
      if (response.state === 'rejected') {
        throw new Error(
          response.failure_message ?? response.failure_code ?? 'Revocation was rejected',
        )
      }
      setRevokeOpen(false)
      setRevokeReason('')
      toast.success('Coordination authorization revoked. Existing work was preserved.')
    } catch (error) {
      const message = getApiErrorMessage(error, 'Coordination authorization was not revoked')
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
      <header className="space-y-3">
        <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
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
              {overview.dispatch.state === 'paused' ? (
                <Badge variant="secondary">New work paused</Badge>
              ) : null}
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              Who acts, what Forge expects, and why the flow is waiting.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>
              {overview.turn_count} / {overview.max_turns} preparation turns
            </span>
            <span>·</span>
            <span>{dispatchStateLabel(overview.dispatch.state)}</span>
          </div>
        </div>
        {nextAction ? (
          <div
            role="status"
            aria-live="polite"
            className={`rounded-md border p-3 ${nextActionToneClass[nextAction.tone]}`}
          >
            <p className="text-xs font-medium uppercase tracking-wide opacity-80">
              Next step · {nextAction.owner}
            </p>
            <p className="mt-1 break-words text-sm font-semibold">{nextAction.summary}</p>
            <p className="mt-1 break-words text-sm opacity-90">{nextAction.detail}</p>
            <div className="mt-3 flex flex-wrap gap-2">
              {nextAction.kind === 'request' && coordinatorAvailable && !batchScopedCoordination ? (
                <Button
                  size="sm"
                  disabled={authorize.isPending || requestTurn.isPending || !objective.trim()}
                  onClick={() => void requestPlan()}
                >
                  {authorize.isPending || requestTurn.isPending
                    ? 'Starting coordinator…'
                    : mode === 'proposal'
                      ? 'Prepare proposal'
                      : 'Start task'}
                </Button>
              ) : null}
              {nextAction.kind === 'resume' ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={createIntervention.isPending}
                  onClick={() => void resumePausedTask()}
                >
                  {createIntervention.isPending ? 'Resuming…' : 'Resume Task'}
                </Button>
              ) : null}
              {nextAction.kind === 'manual' ||
              nextAction.kind === 'blocked' ||
              nextAction.kind === 'authorize' ? (
                <Button size="sm" variant="outline" onClick={onOpenAdvancedWork}>
                  Open Advanced work
                </Button>
              ) : null}
              {latestRequestIsReviewFindings &&
              (reworkAutomationQuery.isError || workUnitsQuery.isError) ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={reworkAutomationQuery.isFetching || workUnitsQuery.isFetching}
                  onClick={() => {
                    if (reworkAutomationQuery.isError) void reworkAutomationQuery.refetch()
                    if (workUnitsQuery.isError) void workUnitsQuery.refetch()
                  }}
                >
                  Retry continuation evidence
                </Button>
              ) : null}
            </div>
          </div>
        ) : null}
      </header>

      {overview.mode !== 'manual' && !overview.revoked_at && overview.can_authorize ? (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border-subtle bg-muted/20 p-3">
          <p className="min-w-0 text-sm text-muted-foreground">
            Revoking stops automatic preparation and task dispatch under this authorization.
            Existing plans, WorkUnits, reviews, and running Executions are preserved.
          </p>
          <Button
            size="sm"
            variant="outline"
            disabled={createIntervention.isPending}
            onClick={() => setRevokeOpen(true)}
          >
            Revoke authorization…
          </Button>
        </div>
      ) : null}

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
              {coordinatorRole ? `${roleLabel(coordinatorRole)} · ` : ''}
              {overview.coordinator.harness ?? 'harness unavailable'} · orchestration
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
            {overview.blockers.map((blocker, index) => {
              const blockerRole = blocker.task_role_id
                ? overview.team.find((role) => role.id === blocker.task_role_id)
                : undefined
              return (
                <li key={`${blocker.code}:${index}`} className="min-w-0 break-words">
                  {readableCoordinationCode(blocker.code)}
                  {blocker.actor_ref ? (
                    <div className="mt-1">
                      <ActorIdentity
                        actor={blocker.actor_ref}
                        agents={agents}
                        members={members}
                        daemons={daemons}
                      />
                    </div>
                  ) : null}
                  <details className="mt-1 text-xs text-muted-foreground">
                    <summary className="w-fit cursor-pointer">Technical details</summary>
                    <p className="mt-1 break-all">{blocker.message}</p>
                    {blockerRole ? <p>TaskRole: {roleLabel(blockerRole)}</p> : null}
                    {blocker.task_role_id ? (
                      <p>
                        TaskRole reference:{' '}
                        <code className="break-all">{blocker.task_role_id}</code>
                      </p>
                    ) : null}
                  </details>
                </li>
              )
            })}
          </ul>
        </div>
      ) : null}

      {actionError ? (
        <div
          role="alert"
          className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-destructive/40 p-3 text-sm text-destructive"
        >
          <span className="min-w-0 break-words">{actionError}</span>
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                setActionError(null)
                void query.refetch()
              }}
            >
              Refresh state
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setActionError(null)}>
              Dismiss
            </Button>
          </div>
        </div>
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

      {batchOwnsActiveAuthorization ? (
        <div
          role="status"
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border-subtle bg-muted/20 p-3"
        >
          <p className="min-w-0 text-sm text-muted-foreground">
            The Project batch owns this Task coordination authorization while it is running or
            paused. To switch to standalone coordination, explicitly revoke this Task authorization
            or revoke/complete the batch, then authorize again here. The old batch grant becomes
            stale and admits no more work.
          </p>
          <Link
            to="/projects/$projectId/plan"
            params={{ projectId: task.project_id }}
            className="inline-flex h-8 items-center justify-center rounded-md px-[9px] text-xs font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            Open Project plan
          </Link>
        </div>
      ) : null}

      {canEditCoordinationAuthorization ? (
        <div className="space-y-4 rounded-md border p-3">
          <div>
            <h3 className="font-medium">
              {overview.mode === 'manual' || overview.revoked_at
                ? 'Authorize a Task coordinator'
                : 'Change Task coordination authorization'}
            </h3>
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
                Automatic coordination requires an active Codex or Claude Code Agent in a Task role
                with orchestration capability. You can change this Task&apos;s roles in Advanced
                work, or reuse a Project team template so new Tasks start with eligible members.
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                <Button size="sm" variant="outline" onClick={onOpenAdvancedWork}>
                  Open Advanced work
                </Button>
                <Link
                  to="/projects/$projectId/settings/$tab"
                  params={{ projectId: task.project_id, tab: 'workers' }}
                  className="inline-flex h-8 items-center justify-center rounded-md px-[9px] text-xs font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  Manage Project team &amp; templates
                </Link>
              </div>
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
            className={overview.mode === 'manual' ? 'hidden' : undefined}
          >
            {authorize.isPending ? 'Saving authorization…' : 'Save authorization'}
          </Button>
        </div>
      ) : null}

      {plannedModeIsAuthorized && turnBudgetExhausted ? (
        <div
          role="status"
          className="rounded-md border border-amber-400/60 bg-amber-50 p-3 text-sm text-amber-950 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100"
        >
          <p className="font-medium">Preparation-turn limit reached</p>
          <p className="mt-1">
            {batchScopedCoordination &&
            overview.project_plan_batch_scope?.blocker ===
              'coordination:coordination.budget_exhausted' ? (
              nextAction?.detail
            ) : (
              <>
                Forge recorded {overview.turn_count} of {overview.max_turns} preparation turns. No
                further automatic turn will be admitted. Existing plans and WorkUnits are preserved;
                continue through Advanced work or ask a Project owner to decide a new Task budget.
              </>
            )}
          </p>
          <Button size="sm" variant="outline" className="mt-3" onClick={onOpenAdvancedWork}>
            Continue with Advanced work
          </Button>
        </div>
      ) : null}

      {(overview.can_request || (overview.can_authorize && selectedCoordinator)) &&
      !batchScopedCoordination &&
      !turnBudgetExhausted &&
      coordinatorAvailable ? (
        <form
          id="coordination-start-form"
          onSubmit={(event) => void requestPlan(event)}
          className="space-y-4 rounded-md border p-3"
        >
          <div>
            <h3 className="font-medium">Start from the Task objective</h3>
            <p className="mt-1 text-sm text-muted-foreground">
              Starting authorizes this coordinator and the selected mode for this Task. The
              coordinator creates the work breakdown and assigns the team; WorkUnits are prepared
              automatically within these boundaries.
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
          <Button
            size="sm"
            type="submit"
            disabled={!objective.trim() || requestTurn.isPending || authorize.isPending}
          >
            {requestTurn.isPending || authorize.isPending
              ? 'Starting coordinator…'
              : mode === 'proposal'
                ? 'Prepare proposal'
                : 'Start task'}
          </Button>
        </form>
      ) : null}

      {plannedModeIsAuthorized &&
      overview.can_request &&
      !batchScopedCoordination &&
      !turnBudgetExhausted &&
      !coordinatorAvailable ? (
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
              {reviewFindingsProjection
                ? 'Latest review-findings request'
                : 'Latest preparation request'}
            </h3>
            <Badge variant="outline">
              {reviewFindingsProjection ? 'Review findings' : 'WorkUnit plan'}
            </Badge>
            <Badge
              variant={
                request.state === 'failed' || request.state === 'uncertain'
                  ? 'destructive'
                  : 'outline'
              }
            >
              {reviewFindingsProjection?.stateLabel ?? requestStateLabels[request.state]}
            </Badge>
          </div>
          {reviewFindingsProjection ? (
            <div className="mt-2 space-y-2 rounded-md border bg-background/60 p-3 text-sm">
              <p className="font-medium">
                Request kind · Review findings · {reviewFindingsProjection.stateLabel}
              </p>
              <div className="space-y-1 break-all text-xs text-muted-foreground">
                <p>
                  {reviewFindingsProjection.reportId
                    ? 'Exact ReviewReport source is attached.'
                    : 'Exact ReviewReport source is unavailable.'}
                </p>
                <p>
                  {reviewFindingsProjection.producerExecutionId
                    ? 'Reviewer producer Execution source is attached.'
                    : 'Reviewer producer Execution source is unavailable.'}
                </p>
                {request.execution_id ? <p>Coordinator disposition is recorded.</p> : null}
              </div>
              {reviewFindingsProjection.triage ? (
                <p className="text-xs text-muted-foreground">
                  The coordinator recorded a disposition for this exact report. This does not
                  approve the ReviewReport, satisfy a Gate, or set TaskLifecycle to done.
                </p>
              ) : null}
              {reviewFindingsProjection.blockers.length ? (
                <ul className="list-disc space-y-1 pl-5 text-xs text-amber-800 dark:text-amber-200">
                  {reviewFindingsProjection.blockers.map((blocker) => (
                    <li key={blocker}>{blocker}</li>
                  ))}
                </ul>
              ) : null}
              <details className="text-xs text-muted-foreground">
                <summary className="w-fit cursor-pointer">Full immutable source references</summary>
                <div className="mt-2 space-y-1 break-all">
                  <p>Request ID: {request.id}</p>
                  <p>ReviewReport: {reviewFindingsProjection.reportId ?? 'unavailable'}</p>
                  <p>Report digest: {reviewFindingsProjection.reportDigest ?? 'unavailable'}</p>
                  <p>
                    Reviewer producer Execution:{' '}
                    {reviewFindingsProjection.producerExecutionId ?? 'unavailable'}
                  </p>
                  {request.execution_id ? <p>Coordinator Execution: {request.execution_id}</p> : null}
                  <div className="flex flex-wrap gap-2">
                    {reviewFindingsProjection.reportId ? (
                      <CopyReferenceButton
                        value={reviewFindingsProjection.reportId}
                        label="ReviewReport ID"
                      />
                    ) : null}
                    {reviewFindingsProjection.reportDigest ? (
                      <CopyReferenceButton
                        value={reviewFindingsProjection.reportDigest}
                        label="ReviewReport digest"
                      />
                    ) : null}
                  </div>
                </div>
              </details>
            </div>
          ) : (
            <p className="mt-2 break-words text-sm">{request.brief.objective}</p>
          )}
          {!reviewFindingsProjection && request.brief.constraints ? (
            <p className="mt-1 whitespace-pre-wrap break-words text-sm text-muted-foreground">
              Constraints: {request.brief.constraints}
            </p>
          ) : null}
          {!reviewFindingsProjection && request.brief.acceptance_criteria.length ? (
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
                dispatch: {dispatchStateLabel(overview.dispatch.state)}.
              </p>
              {overview.dispatch.state === 'idle' &&
              plannedModeIsAuthorized &&
              overview.can_authorize ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={
                    !request || materialize.isPending || planQuery.isError || requestQuery.isError
                  }
                  onClick={() => void materializePlan()}
                >
                  {materialize.isPending ? 'Preparing assigned work…' : 'Retry work preparation'}
                </Button>
              ) : null}
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
              {overview.dispatch.state === 'paused' ? (
                <>
                  <p className="text-sm text-muted-foreground">
                    The Task is paused, so Forge will not materialize this plan yet. Resume the Task
                    first; no worker is started by this plan alone.
                  </p>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={createIntervention.isPending}
                    onClick={() => void resumePausedTask()}
                  >
                    {createIntervention.isPending ? 'Resuming…' : 'Resume Task to continue'}
                  </Button>
                </>
              ) : (
                <>
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
                </>
              )}
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

      {overview.review_triage?.length ? (
        <section className="space-y-3 rounded-lg border p-4">
          <div>
            <h3 className="font-medium">Coordinator review triage history</h3>
            <p className="mt-1 text-xs text-muted-foreground">
              Each disposition is bound to one ReviewReport. Triage does not change its verdict,
              satisfy a Gate, or complete the Task.
            </p>
          </div>
          {overview.review_triage.map((triage) => (
            <article
              key={triage.review_report_artifact_id}
              className="rounded-md border p-3 text-sm"
            >
              <p className="font-medium">
                {triage.disposition === 'correct'
                  ? 'Findings confirmed for correction'
                  : triage.disposition === 'recheck'
                    ? 'Fresh review WorkUnit recorded'
                    : 'Human decision requested'}
              </p>
              <p className="mt-2 whitespace-pre-wrap">{triage.rationale}</p>
              <p className="mt-2 whitespace-pre-wrap text-muted-foreground">
                {triage.instructions}
              </p>
              <details className="mt-2 text-xs text-muted-foreground">
                <summary className="cursor-pointer">Exact report and decision provenance</summary>
                <p className="mt-1 break-all">Report: {triage.review_report_artifact_id}</p>
                <p className="break-all">Coordinator Execution: {triage.execution_id}</p>
                {triage.review_work_unit_id ? (
                  <p className="break-all">New review WorkUnit: {triage.review_work_unit_id}</p>
                ) : null}
              </details>
            </article>
          ))}
        </section>
      ) : null}
      <TaskQualityRequestsPanel
        task={task}
        coordination={overview}
        agents={agents}
        members={members}
        daemons={daemons}
      />
      <TaskInterventionsPanel task={task} coordination={overview} />
      <TaskAutonomyLimitsPanel taskId={task.id} canManage={canManageLimits} />

      <Dialog open={revokeOpen} onOpenChange={setRevokeOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Revoke coordination authorization?</DialogTitle>
            <DialogDescription>
              Forge stops automatic preparation and task dispatch under this authorization. Existing
              plans, WorkUnits, reviews, and running Executions are preserved. Running Executions
              are not silently stopped. You can authorize coordination again later.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Label htmlFor="coordination-revoke-reason">Reason (optional)</Label>
            <Textarea
              id="coordination-revoke-reason"
              value={revokeReason}
              onChange={(event) => setRevokeReason(event.target.value)}
              placeholder="Why should automatic coordination stop?"
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={createIntervention.isPending}
              onClick={() => setRevokeOpen(false)}
            >
              Keep authorization
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={createIntervention.isPending}
              onClick={() => void revokeAuthorization()}
            >
              {createIntervention.isPending ? 'Revoking…' : 'Revoke authorization'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}
