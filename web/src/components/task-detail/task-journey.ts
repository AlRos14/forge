import type {
  GateResponse,
  ReviewLineResponse,
  Task,
  TaskCoordinationOverviewResponse,
  TaskDispatchIntentResponse,
  TaskInterventionResponse,
  TaskQualityRequestResponse,
  WorkUnitResponse,
} from '@/types/generated'
import {
  dispatchStateLabel,
  deriveCoordinationNextAction,
  isTurnBudgetExhausted,
  readableCoordinationCode,
} from './task-coordination-status'

export type JourneyStageStatus = 'done' | 'current' | 'waiting' | 'blocked' | 'not_started'

export type JourneyStage = {
  id: 'objective' | 'plan' | 'execution' | 'quality' | 'delivery'
  title: string
  status: JourneyStageStatus
  summary: string
  detail: string
}

export type JourneyInput = {
  task: Task
  coordination: TaskCoordinationOverviewResponse
  dispatch: TaskDispatchIntentResponse | undefined
  workUnits: WorkUnitResponse[]
  reviewLines: ReviewLineResponse[]
  gates: GateResponse[]
  qualityRequests: TaskQualityRequestResponse[]
  interventions: TaskInterventionResponse[]
}

export type JourneyProjection = {
  stages: JourneyStage[]
  nextOwner: string
  nextSummary: string
  nextDetail: string
  escalatedToHuman: boolean
  blockers: string[]
  activeActors: Array<NonNullable<WorkUnitResponse['assigned_actor']>>
}

function plural(count: number, singular: string, pluralValue = `${singular}s`) {
  return `${count} ${count === 1 ? singular : pluralValue}`
}

export function deriveJourney(input: JourneyInput): JourneyProjection {
  const {
    task,
    coordination,
    dispatch,
    workUnits,
    reviewLines,
    gates,
    qualityRequests,
    interventions,
  } = input
  const activeUnits = workUnits.filter((unit) => unit.status === 'open')
  const completedUnits = workUnits.filter((unit) => unit.status === 'completed')
  const reviewRoleNames = new Set(
    coordination.team
      .filter((role) => role.capabilities.includes('formal_review'))
      .map((role) => role.role),
  )
  const openExecutionUnits = activeUnits.filter((unit) => !reviewRoleNames.has(unit.role))
  const paused = dispatch?.state === 'paused' || coordination.dispatch.state === 'paused'
  const cancelled =
    task.lifecycle.state === 'cancelled' || coordination.dispatch.state === 'cancelled'
  const done = task.lifecycle.state === 'done'
  const plan = coordination.latest_plan
  const request = coordination.latest_request
  const next = deriveCoordinationNextAction(coordination, request, plan)
  const escalatedQuality = qualityRequests.filter((item) => item.state === 'requires_human')
  const escalatedInterventions = interventions.filter((item) => item.state === 'requires_human')
  const escalatedToHuman = escalatedQuality.length > 0 || escalatedInterventions.length > 0
  const openReview = reviewLines.filter((line) => line.latest_verdict !== 'pass')
  const maxRevision = reviewLines.reduce((max, line) => Math.max(max, line.revision), 0)
  const activeRoleMembers = coordination.team.flatMap((role) =>
    role.members.filter((member) => member.status === 'active'),
  )
  const activeActors = [
    ...new Map(
      activeUnits
        .map((unit) => unit.assigned_actor)
        .filter((actor): actor is NonNullable<typeof actor> => Boolean(actor))
        .map((actor) => [`${actor.kind}:${actor.id}`, actor]),
    ).values(),
  ]

  const objective: JourneyStage = {
    id: 'objective',
    title: 'Objective and team',
    status: coordination.team.length ? 'done' : 'blocked',
    summary: coordination.team.length
      ? `${plural(coordination.team.length, 'role')} · ${plural(activeRoleMembers.length, 'active member')}`
      : 'No Task role is available',
    detail: coordination.coordinator
      ? `Coordinator: ${coordination.coordinator.display_name} (${coordination.coordinator.harness ?? 'harness unavailable'})`
      : 'No coordinator is saved. You can still use manual planning.',
  }

  let planStage: JourneyStage
  if (cancelled) {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'blocked',
      summary: 'Task cancelled',
      detail: 'No new preparation or materialization is admitted.',
    }
  } else if (coordination.revoked_at) {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'blocked',
      summary: 'Authorization revoked',
      detail: 'Existing plans and WorkUnits are preserved; re-authorize to prepare more work.',
    }
  } else if (coordination.mode === 'manual') {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: workUnits.length ? 'done' : 'current',
      summary: workUnits.length ? 'Manual plan recorded' : 'Manual planning',
      detail: workUnits.length
        ? `${plural(workUnits.length, 'WorkUnit')} prepared outside automatic coordination.`
        : 'Create or edit roles and WorkUnits in Advanced work, or authorize guided preparation.',
    }
  } else if (plan?.materialization) {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'done',
      summary: 'Plan materialized',
      detail: `${plural(plan.materialization.work_unit_ids.length, 'WorkUnit')} created from the exact approved plan.`,
    }
  } else if (plan?.state === 'proposed') {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'current',
      summary: 'Plan ready for your review',
      detail: 'Inspect the exact digest, then approve or reject. No WorkUnit exists yet.',
    }
  } else if (plan?.state === 'approved') {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'current',
      summary: paused ? 'Approved; waiting for resume' : 'Approved; materialization pending',
      detail: paused
        ? 'Forge will not create WorkUnits while the Task is paused.'
        : 'Materialize the exact approved plan to create WorkUnits and the task launch intent.',
    }
  } else if (isTurnBudgetExhausted(coordination)) {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'blocked',
      summary: 'Preparation-turn limit reached',
      detail: `${coordination.turn_count} of ${coordination.max_turns} turns used. Continue through Advanced work or ask a Project owner to decide how to proceed.`,
    }
  } else if (request?.state === 'requested' || request?.state === 'running') {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'waiting',
      summary: 'Coordinator is preparing a plan',
      detail: 'One bounded read-only turn is admitted. No WorkUnit or worker exists yet.',
    }
  } else if (request?.state === 'failed' || request?.state === 'uncertain') {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'blocked',
      summary:
        request.state === 'uncertain' ? 'Provider outcome is uncertain' : 'Preparation failed',
      detail: request.failure_message
        ? request.failure_message
        : 'Inspect the recorded request; Forge does not retry an uncertain provider start automatically.',
    }
  } else if (coordination.can_request) {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'current',
      summary: 'Ready to prepare from the objective',
      detail: 'Describe the outcome; the saved Task team prepares an inspectable breakdown.',
    }
  } else {
    planStage = {
      id: 'plan',
      title: 'Plan',
      status: 'not_started',
      summary: 'No plan is recorded',
      detail:
        coordination.blockers[0] != null
          ? readableCoordinationCode(coordination.blockers[0].code)
          : 'Coordinate the objective or create WorkUnits manually.',
    }
  }

  let executionStage: JourneyStage
  if (!workUnits.length) {
    executionStage = {
      id: 'execution',
      title: 'Execution',
      status: planStage.status === 'done' ? 'waiting' : 'not_started',
      summary: 'No WorkUnit exists',
      detail: 'A worker cannot start without an allocated WorkUnit and an eligible Actor.',
    }
  } else if (paused) {
    executionStage = {
      id: 'execution',
      title: 'Execution',
      status: 'blocked',
      summary: 'New work is paused',
      detail: `${plural(activeUnits.length, 'open WorkUnit')} remain; work already running may continue. Resume to admit new work.`,
    }
  } else if (cancelled) {
    executionStage = {
      id: 'execution',
      title: 'Execution',
      status: 'blocked',
      summary: 'Dispatch cancelled',
      detail: 'No new worker will start. Existing evidence remains in history.',
    }
  } else if (activeUnits.length && !openExecutionUnits.length) {
    executionStage = {
      id: 'execution',
      title: 'Execution',
      status: 'done',
      summary: `${plural(completedUnits.length, 'WorkUnit')} complete`,
      detail:
        'No non-review WorkUnit remains open. Formal review WorkUnits and their exact verdicts are tracked under Quality and correction.',
    }
  } else if (activeUnits.length) {
    executionStage = {
      id: 'execution',
      title: 'Execution',
      status: 'current',
      summary: `${plural(activeUnits.length, 'open WorkUnit')} · ${completedUnits.length} complete`,
      detail: `${dispatchStateLabel(coordination.dispatch.state)}. Exact assigned Actors and blockers are listed below.`,
    }
  } else {
    executionStage = {
      id: 'execution',
      title: 'Execution',
      status: 'done',
      summary: `${plural(completedUnits.length, 'WorkUnit')} complete`,
      detail: 'Technical completion is not by itself review approval or business acceptance.',
    }
  }

  const routeEscalation = escalatedQuality.find(
    (item) => item.quality_route?.escalation,
  )?.quality_route.escalation

  let qualityStage: JourneyStage
  if (escalatedToHuman) {
    qualityStage = {
      id: 'quality',
      title: 'Quality and correction',
      status: 'blocked',
      summary: routeEscalation ? 'Quality route escalated to a human' : 'Escalated to a human',
      detail: routeEscalation
        ? routeEscalation.reason
        : escalatedQuality.length > 0
          ? `${plural(escalatedQuality.length, 'quality request')} needs a decision before another automatic route is applied.`
          : 'An intervention recorded that a human decision is required.',
    }
  } else if (!workUnits.length) {
    qualityStage = {
      id: 'quality',
      title: 'Quality and correction',
      status: 'not_started',
      summary: 'Nothing to review yet',
      detail: 'Formal review attaches to an exact WorkUnit result.',
    }
  } else if (openReview.length) {
    qualityStage = {
      id: 'quality',
      title: 'Quality and correction',
      status: 'current',
      summary:
        maxRevision > 1
          ? `Review revision ${String(maxRevision)} is open`
          : 'Review or correction is open',
      detail:
        openReview.length === reviewLines.length
          ? 'An exact result is waiting for an independent reviewer or a correction.'
          : `${plural(openReview.length, 'review line')} still need a passing independent review.`,
    }
  } else if (reviewLines.length) {
    qualityStage = {
      id: 'quality',
      title: 'Quality and correction',
      status: 'done',
      summary: 'Independent review passed',
      detail:
        maxRevision > 1
          ? `The current result passed after ${String(maxRevision)} review revision${maxRevision === 1 ? '' : 's'}.`
          : 'The current exact result has a passing ReviewReport.',
    }
  } else {
    qualityStage = {
      id: 'quality',
      title: 'Quality and correction',
      status: 'waiting',
      summary: 'No review line is recorded',
      detail: 'Create an exact reviewer subject before formal review can run.',
    }
  }

  let deliveryStage: JourneyStage
  if (done) {
    deliveryStage = {
      id: 'delivery',
      title: 'Delivery',
      status: 'done',
      summary: 'Task done',
      detail: 'The aggregate TaskLifecycle is done. Results and evidence remain available.',
    }
  } else if (cancelled) {
    deliveryStage = {
      id: 'delivery',
      title: 'Delivery',
      status: 'blocked',
      summary: 'Task cancelled',
      detail: 'No delivery will be admitted from this Task.',
    }
  } else if (task.lifecycle.state === 'ready_to_merge') {
    deliveryStage = {
      id: 'delivery',
      title: 'Delivery',
      status: 'current',
      summary: 'Ready for integration checks',
      detail: `Evaluate the current Gate${gates.length === 1 ? '' : 's'} against the exact passing result before integration.`,
    }
  } else if (gates.length) {
    deliveryStage = {
      id: 'delivery',
      title: 'Delivery',
      status: 'waiting',
      summary: `${plural(gates.length, 'Gate')} recorded`,
      detail: 'A Gate evaluates exact immutable inputs; it does not run tests, review code, or merge.',
    }
  } else {
    deliveryStage = {
      id: 'delivery',
      title: 'Delivery',
      status: 'not_started',
      summary: 'Not ready for delivery',
      detail:
        'Delivery follows the exact passing review and Gate inputs. The current Task lifecycle remains visible below.',
    }
  }

  const blockers = [
    ...coordination.blockers.map((blocker) => readableCoordinationCode(blocker.code)),
    ...(paused ? ['The Task dispatch is paused.'] : []),
    ...(escalatedQuality.length
      ? [
          routeEscalation
            ? `Quality route escalated: ${routeEscalation.reason}`
            : `${plural(escalatedQuality.length, 'quality request')} escalated to a human.`,
        ]
      : []),
  ]
  const uniqueBlockers = [...new Set(blockers)]
  const humanEscalation = escalatedToHuman
    ? {
        nextOwner: 'You',
        nextSummary: 'A quality decision needs a human.',
        nextDetail:
          'Review the escalated quality request below. Forge will not apply another automatic route while it is escalated.',
      }
    : done
      ? {
          nextOwner: 'Nobody',
          nextSummary: 'The Task is done.',
          nextDetail:
            'No further action is required. Results, review evidence, and Gate facts remain available.',
        }
      : null

  return {
    stages: [objective, planStage, executionStage, qualityStage, deliveryStage],
    nextOwner: humanEscalation?.nextOwner ?? next.owner,
    nextSummary: humanEscalation?.nextSummary ?? next.summary,
    nextDetail: humanEscalation?.nextDetail ?? next.detail,
    escalatedToHuman,
    blockers: uniqueBlockers,
    activeActors,
  }
}
