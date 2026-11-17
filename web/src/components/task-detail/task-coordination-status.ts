import type {
  TaskCoordinationOverviewResponse,
  TaskCoordinationPlanResponse,
  TaskCoordinationRequestResponse,
  TaskCoordinationTeamRoleResponse,
  TaskDispatchIntentResponse,
  TaskReworkAutomationIntentResponse,
  WorkUnitResponse,
} from '@/types/generated'
import {
  deriveReviewFindingsProjection,
  type ReviewFindingsDataState,
  type ReviewFindingsProjection,
} from './task-review-triage'

export type CoordinationActionKind =
  | 'authorize'
  | 'request'
  | 'approve'
  | 'materialize'
  | 'resume'
  | 'revoke'
  | 'manual'
  | 'wait'
  | 'blocked'
  | 'done'

export type CoordinationNextAction = {
  summary: string
  detail: string
  owner: string
  kind: CoordinationActionKind
  tone: 'action' | 'wait' | 'blocked' | 'done' | 'info'
}

const blockerLabels: Record<string, string> = {
  'coordination.authorization_revoked':
    'Coordination authorization is revoked. Existing plans and WorkUnits are preserved.',
  'coordination.authorization_stale':
    'The saved Task team changed. Review the team and authorize coordination again.',
  'coordination.budget_exhausted':
    'The Task preparation-turn limit is exhausted. Forge will not accept another automatic turn.',
  'coordination.coordinator_capability_missing':
    'The selected role no longer has orchestration capability.',
  'coordination.coordinator_missing':
    'No Agent coordinator is saved for this Task. Choose one with orchestration capability.',
  'coordination.coordinator_not_member':
    'The saved coordinator is no longer an active member of its Task role.',
  'coordination.not_authorized':
    'Your current Project role cannot change this Task team or coordination.',
  'coordination.paused':
    'The Task is paused. Materialization and task launch wait for resume; an already-running read-only turn may finish.',
  'coordination.task_cancelled':
    'The Task is cancelled. No new preparation turn, materialization, or dispatch will be admitted.',
}

const dispatchLabels: Record<TaskDispatchIntentResponse['state'], string> = {
  idle: 'No dispatch intent',
  running: 'Dispatch accepting work',
  paused: 'Dispatch paused',
  cancelled: 'Dispatch cancelled',
}

export function readableCoordinationCode(code: string): string {
  return (
    blockerLabels[code] ??
    (code.replaceAll('.', ' ').replaceAll('_', ' ').trim() || 'Coordination blocker')
  )
}

export function coordinationRoleLabel(role: TaskCoordinationTeamRoleResponse): string {
  return role.display_label?.trim() || role.role.replaceAll('_', ' ')
}

export function dispatchStateLabel(state: TaskDispatchIntentResponse['state']): string {
  return dispatchLabels[state]
}

export function isTurnBudgetExhausted(overview: TaskCoordinationOverviewResponse): boolean {
  return overview.max_turns > 0 && overview.turn_count >= overview.max_turns
}

export function isProjectBatchScopedCoordination(
  overview: TaskCoordinationOverviewResponse,
): boolean {
  return overview.project_plan_batch_scope !== null
}

export function canEditTaskCoordinationAuthorization(
  overview: TaskCoordinationOverviewResponse,
): boolean {
  const batchScope = overview.project_plan_batch_scope
  return (
    overview.can_authorize &&
    (!batchScope || ['revoked', 'completed'].includes(batchScope.batch_state))
  )
}

export function isQualityBudgetExhausted(overview: TaskCoordinationOverviewResponse): boolean {
  return (
    overview.max_quality_requests > 0 &&
    overview.quality_request_count >= overview.max_quality_requests
  )
}

function hasCoordinatorProblem(overview: TaskCoordinationOverviewResponse): boolean {
  return overview.blockers.some((blocker) =>
    [
      'coordination.coordinator_missing',
      'coordination.coordinator_not_member',
      'coordination.coordinator_capability_missing',
    ].includes(blocker.code),
  )
}

/**
 * Presentation-only projection of durable coordination facts. It deliberately
 * does not infer progress from Execution counts or from local UI state.
 */
export function deriveCoordinationNextAction(
  overview: TaskCoordinationOverviewResponse,
  request: TaskCoordinationRequestResponse | null,
  plan: TaskCoordinationPlanResponse | null,
  reviewContext: {
    automationIntents?: TaskReworkAutomationIntentResponse[]
    automationIntentsState?: ReviewFindingsDataState
    workUnits?: WorkUnitResponse[]
    workUnitsState?: ReviewFindingsDataState
  } = {},
): CoordinationNextAction {
  if (overview.lifecycle.state === 'done') {
    return {
      summary: 'The Task is done.',
      detail:
        'No further coordination or delivery action is required. Results, review evidence, and Gate facts remain available.',
      owner: 'Nobody',
      kind: 'done',
      tone: 'done',
    }
  }

  if (overview.lifecycle.state === 'merging') {
    return {
      summary: 'Task integration is in progress.',
      detail:
        'Forge owns the serialized integration operation. Wait for its exact result; do not start another merge.',
      owner: 'Forge',
      kind: 'wait',
      tone: 'wait',
    }
  }

  if (overview.lifecycle.state === 'ready_to_merge') {
    return {
      summary: 'Task is ready for integration.',
      detail:
        'The current TaskLifecycle is ready_to_merge. Continue through the existing exact integration operation. Review triage does not change that state or admit integration.',
      owner: 'You',
      kind: 'wait',
      tone: 'action',
    }
  }

  if (overview.lifecycle.state === 'cancelled' || overview.dispatch.state === 'cancelled') {
    return {
      summary: 'This Task is cancelled.',
      detail:
        'No new preparation, materialization, or dispatch is admitted. Existing plans, WorkUnits, reviews, and evidence remain in history.',
      owner: 'Nobody',
      kind: 'done',
      tone: 'done',
    }
  }

  const reviewFindings: ReviewFindingsProjection | null = deriveReviewFindingsProjection({
    overview,
    request,
    automationIntents: reviewContext.automationIntents,
    automationIntentsState: reviewContext.automationIntentsState,
    workUnits: reviewContext.workUnits,
    workUnitsState: reviewContext.workUnitsState,
  })
  if (reviewFindings) return reviewFindings.action

  if (overview.revoked_at) {
    return {
      summary: 'Coordination authorization is revoked.',
      detail:
        'Existing plans, WorkUnits, and running Executions are not deleted or stopped. Authorize coordination again to prepare more work, or continue through Advanced work.',
      owner: 'You',
      kind: 'authorize',
      tone: 'action',
    }
  }

  if (overview.mode === 'manual') {
    return {
      summary: 'Manual planning is active.',
      detail:
        'Create or edit roles and WorkUnits in Advanced work. Guided preparation is available after you authorize a coordinator and mode.',
      owner: 'You',
      kind: 'manual',
      tone: 'info',
    }
  }

  if (overview.blockers.some((blocker) => blocker.code === 'coordination.authorization_stale')) {
    return {
      summary: 'The saved Task team changed.',
      detail:
        'Forge will not use a stale team snapshot. Review the current roles and members, then save the authorization again.',
      owner: 'You',
      kind: 'authorize',
      tone: 'blocked',
    }
  }

  if (hasCoordinatorProblem(overview)) {
    return {
      summary: 'The Task coordinator needs attention.',
      detail:
        'Review the current Task team, choose an active Agent in a role with orchestration capability, and save the authorization again.',
      owner: 'You',
      kind: 'authorize',
      tone: 'blocked',
    }
  }

  if (isTurnBudgetExhausted(overview)) {
    const batchScope = overview.project_plan_batch_scope
    if (batchScope?.blocker === 'coordination:coordination.budget_exhausted') {
      if (batchScope.batch_state === 'running') {
        return {
          summary: 'The Project batch is waiting for a preparation turn.',
          detail:
            'A Project owner or admin can increase max_turns in Autonomy limits below. The scheduler retries this Task on its next tick only while the same batch is running and its exact authorization, team, and prerequisite checks pass.',
          owner: 'You',
          kind: 'blocked',
          tone: 'blocked',
        }
      }
      if (batchScope.batch_state === 'paused') {
        return {
          summary: 'The Project batch is paused.',
          detail:
            'Increasing max_turns will not restart a paused batch. Resume this exact batch before the scheduler can retry the Task.',
          owner: 'You',
          kind: 'blocked',
          tone: 'blocked',
        }
      }
      return {
        summary: `The Project batch is ${batchScope.batch_state}.`,
        detail:
          'Increasing max_turns does not restart a revoked or completed batch. Its recorded Task and coordination history remains available.',
        owner: 'You',
        kind: 'blocked',
        tone: 'blocked',
      }
    }
    return {
      summary: 'The preparation-turn limit is exhausted.',
      detail: `Forge recorded ${overview.turn_count} of ${overview.max_turns} preparation turns. It will not accept another automatic turn. Continue with existing work or through Advanced work; a new budget requires an explicit product decision.`,
      owner: 'You',
      kind: 'blocked',
      tone: 'blocked',
    }
  }

  if (request?.state === 'requested' || request?.state === 'running') {
    return {
      summary: 'The coordinator Agent is preparing a plan.',
      detail:
        overview.dispatch.state === 'paused'
          ? 'The read-only turn may finish, but materialization and task launch remain paused until you resume.'
          : 'Forge admitted one bounded read-only turn. No WorkUnit or worker is created until a plan is materialized.',
      owner: 'Coordinator Agent',
      kind: 'wait',
      tone: 'wait',
    }
  }

  if (request?.state === 'uncertain') {
    if (overview.project_plan_batch_scope) {
      return {
        summary: 'The Project batch has an uncertain coordination result.',
        detail:
          'Inspect the exact Task request and Project batch blocker. This batch-scoped Task cannot use the generic request route; do not start a duplicate turn.',
        owner: 'You',
        kind: 'blocked',
        tone: 'blocked',
      }
    }
    return {
      summary: 'The provider outcome is uncertain.',
      detail:
        'Forge will not repeat the external start automatically. Inspect the recorded request and host state, then decide whether to request a new bounded turn.',
      owner: 'You',
      kind: 'blocked',
      tone: 'blocked',
    }
  }

  if (request?.state === 'failed') {
    if (overview.project_plan_batch_scope) {
      return {
        summary: 'The Project batch coordination turn failed.',
        detail:
          'Inspect the exact recorded failure and Project batch blocker. This Task cannot use the generic coordination request route.',
        owner: 'You',
        kind: 'blocked',
        tone: 'blocked',
      }
    }
    return {
      summary: 'Plan preparation failed.',
      detail: request.failure_message
        ? `The recorded failure is: ${request.failure_message}`
        : 'Inspect the recorded failure, fix the concrete blocker, and request another turn if authority and budget still allow it.',
      owner: 'You',
      kind: 'blocked',
      tone: 'blocked',
    }
  }

  if (plan?.state === 'proposed') {
    return {
      summary: 'A plan is ready for your review.',
      detail:
        'Inspect the exact plan digest and proposed WorkUnits. Approval records a durable decision; no WorkUnit exists until materialization succeeds.',
      owner: 'You',
      kind: 'approve',
      tone: 'action',
    }
  }

  if (plan?.state === 'approved' && !plan.materialization) {
    if (overview.dispatch.state === 'paused') {
      return {
        summary: 'The plan is approved while the Task is paused.',
        detail:
          'Resume the Task before materializing. Forge then creates WorkUnits and records the normal task launch intent in one deterministic operation.',
        owner: 'You',
        kind: 'resume',
        tone: 'action',
      }
    }
    return {
      summary: 'The plan is approved; WorkUnits are not created yet.',
      detail:
        'Materialize the exact approved plan to create its WorkUnits, dependencies, review subject, and Artifact inputs. Provider start remains a separate dispatch step.',
      owner: 'You',
      kind: 'materialize',
      tone: 'action',
    }
  }

  if (plan?.materialization) {
    if (overview.dispatch.state === 'paused') {
      return {
        summary: 'Work is prepared, but new dispatch is paused.',
        detail:
          'Work already running may continue. Resume to let the dispatcher admit eligible WorkUnits.',
        owner: 'You',
        kind: 'resume',
        tone: 'action',
      }
    }
    return {
      summary: 'Work is prepared; dispatch decides what can start.',
      detail: `${plan.materialization.work_unit_ids.length} WorkUnit${plan.materialization.work_unit_ids.length === 1 ? '' : 's'} exist from the exact plan. ${dispatchStateLabel(overview.dispatch.state)}.`,
      owner: 'Forge dispatcher',
      kind: 'wait',
      tone: 'info',
    }
  }

  const batchScope = overview.project_plan_batch_scope
  if (batchScope) {
    if (batchScope.blocker === 'coordination:coordination.budget_exhausted') {
      if (batchScope.batch_state === 'running') {
        return {
          summary: 'The Project batch will retry this Task.',
          detail:
            'Preparation-turn budget is available again. The scheduler retries on its next tick after rechecking the same batch authorization, Task team, and prerequisites.',
          owner: 'Project scheduler',
          kind: 'wait',
          tone: 'wait',
        }
      }
      if (batchScope.batch_state === 'paused') {
        return {
          summary: 'The Project batch is paused.',
          detail:
            'Increasing max_turns does not resume the batch. Resume this exact batch before the scheduler can retry the Task.',
          owner: 'You',
          kind: 'blocked',
          tone: 'blocked',
        }
      }
      return {
        summary: `The Project batch is ${batchScope.batch_state}.`,
        detail:
          'Increasing max_turns does not restart a revoked or completed batch. Its recorded Task and coordination history remains available.',
        owner: 'You',
        kind: 'blocked',
        tone: 'blocked',
      }
    }
    if (batchScope.batch_state === 'paused') {
      return {
        summary: 'The Project batch is paused.',
        detail:
          'No Task is admitted while this batch is paused. Resolve the recorded blocker and resume this exact batch when it is ready.',
        owner: 'You',
        kind: 'blocked',
        tone: 'blocked',
      }
    }
    if (batchScope.batch_state === 'revoked' || batchScope.batch_state === 'completed') {
      return {
        summary: `The Project batch is ${batchScope.batch_state}.`,
        detail:
          'The scheduler will not admit more Tasks from a revoked or completed batch. Its recorded Task and coordination history remains available.',
        owner: 'You',
        kind: 'blocked',
        tone: 'blocked',
      }
    }
    return {
      summary: 'The Project batch controls the next coordination step.',
      detail:
        'This Task does not accept a generic coordination turn while its authorization is scoped to a Project batch. Resolve the batch blocker before continuing.',
      owner: 'Project scheduler',
      kind: 'wait',
      tone: 'wait',
    }
  }

  if (plan?.state === 'rejected' || plan?.state === 'superseded') {
    return {
      summary:
        plan.state === 'rejected'
          ? 'The proposed plan was rejected.'
          : 'A newer plan superseded this one.',
      detail:
        'The recorded proposal remains in history. Request a new bounded turn with an updated objective, or continue through Advanced work.',
      owner: 'You',
      kind: 'request',
      tone: 'action',
    }
  }

  if (overview.can_request) {
    return {
      summary: 'Describe the outcome and prepare a plan.',
      detail:
        'The saved Task team prepares the breakdown. The request itself creates no WorkUnit and starts no worker.',
      owner: 'You',
      kind: 'request',
      tone: 'action',
    }
  }

  if (overview.blockers.length) {
    return {
      summary: 'Coordination is blocked.',
      detail: readableCoordinationCode(overview.blockers[0].code),
      owner: 'You',
      kind: 'blocked',
      tone: 'blocked',
    }
  }

  return {
    summary: 'Coordination is awaiting the next durable fact.',
    detail:
      'Refresh the Task state. The UI does not infer progress from local state or from the number of Executions.',
    owner: 'Forge',
    kind: 'wait',
    tone: 'info',
  }
}
