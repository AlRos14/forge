import type {
  TaskCoordinationOverviewResponse,
  TaskCoordinationRequestResponse,
  TaskReviewTriageResponse,
  TaskReworkAutomationIntentResponse,
  WorkUnitResponse,
} from '@/types/generated'

export type ReviewFindingsAction = {
  kind: 'authorize' | 'blocked' | 'done' | 'manual' | 'resume' | 'wait'
  tone: 'action' | 'blocked' | 'done' | 'info' | 'wait'
  owner: string
  summary: string
  detail: string
}

export type ReviewFindingsDataState = 'loading' | 'error' | 'loaded'

export type ReviewFindingsProjection = {
  request: TaskCoordinationRequestResponse
  reportId: string | null
  reportDigest: string | null
  producerExecutionId: string | null
  stateLabel: string
  triage: TaskReviewTriageResponse | null
  reworkIntent: TaskReworkAutomationIntentResponse | null
  reviewWorkUnit: WorkUnitResponse | null
  correctionWorkUnit: WorkUnitResponse | null
  action: ReviewFindingsAction
  blockers: string[]
  escalatedToHuman: boolean
}

type ReviewFindingsProjectionInput = {
  overview: TaskCoordinationOverviewResponse
  request: TaskCoordinationRequestResponse | null
  automationIntents?: TaskReworkAutomationIntentResponse[]
  automationIntentsState?: ReviewFindingsDataState
  workUnits?: WorkUnitResponse[]
  workUnitsState?: ReviewFindingsDataState
}

export function isReviewFindingsRequest(
  request: TaskCoordinationRequestResponse | null | undefined,
): boolean {
  return request?.kind === 'review_findings'
}

function readableRequestState(state: string): string {
  if (state === 'triaged') return 'Findings triaged'
  if (state === 'uncertain') return 'Provider outcome uncertain'
  return state.replaceAll('_', ' ')
}

function exactTriage(
  request: TaskCoordinationRequestResponse,
  overview: TaskCoordinationOverviewResponse,
): TaskReviewTriageResponse | null {
  if (
    request.state !== 'triaged' ||
    !request.review_report_artifact_id ||
    !request.execution_id
  ) {
    return null
  }

  // request.execution_id is the coordinator execution; the separate
  // review_producer_execution_id below identifies the reviewer that produced
  // the immutable source report.
  return (
    overview.review_triage?.find(
      (triage) =>
        triage.review_report_artifact_id === request.review_report_artifact_id &&
        triage.execution_id === request.execution_id,
    ) ?? null
  )
}

function action(
  kind: ReviewFindingsAction['kind'],
  tone: ReviewFindingsAction['tone'],
  owner: string,
  summary: string,
  detail: string,
): ReviewFindingsAction {
  return { kind, tone, owner, summary, detail }
}

function unavailableContinuationData(
  state: ReviewFindingsDataState,
  label: string,
): { action: ReviewFindingsAction; blockers: string[]; escalatedToHuman: boolean } | null {
  if (state === 'loaded') return null
  if (state === 'loading') {
    return {
      action: action(
        'wait',
        'wait',
        'Forge',
        `Loading ${label}.`,
        'The durable continuation records are still loading. Forge will not infer that a receipt or WorkUnit is missing.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }
  return {
    action: action(
      'blocked',
      'blocked',
      'You',
      `Could not load ${label}.`,
      'Retry loading the exact continuation records before deciding whether a receipt or WorkUnit is missing.',
    ),
    blockers: [`${label} could not be loaded.`],
    escalatedToHuman: false,
  }
}

function isAuthorizationStale(overview: TaskCoordinationOverviewResponse): boolean {
  return overview.blockers.some((blocker) => blocker.code === 'coordination.authorization_stale')
}

function projectReviewFindingsAction(
  overview: TaskCoordinationOverviewResponse,
  request: TaskCoordinationRequestResponse,
  triage: TaskReviewTriageResponse | null,
  automationIntents: TaskReworkAutomationIntentResponse[],
  automationIntentsState: ReviewFindingsDataState,
  workUnits: WorkUnitResponse[],
  workUnitsState: ReviewFindingsDataState,
): { action: ReviewFindingsAction; blockers: string[]; escalatedToHuman: boolean } {
  if (overview.lifecycle.state === 'done') {
    return {
      action: action(
        'done',
        'done',
        'Nobody',
        'The Task is done.',
        'The review request and disposition remain in history. The Task lifecycle records the completed delivery.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  if (overview.lifecycle.state === 'merging') {
    return {
      action: action(
        'wait',
        'wait',
        'Forge',
        'Task integration is in progress.',
        'Wait for the exact integration result. The review triage does not complete the Task.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  if (overview.lifecycle.state === 'ready_to_merge') {
    return {
      action: action(
        'wait',
        'action',
        'You',
        'Task is ready for integration.',
        'The current TaskLifecycle is ready_to_merge. Continue through the existing exact integration operation. Review triage does not alter that authority or admit integration.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  if (overview.lifecycle.state === 'cancelled' || overview.dispatch.state === 'cancelled') {
    return {
      action: action(
        'blocked',
        'blocked',
        'Nobody',
        'The Task is cancelled.',
        'The recorded ReviewReport and triage remain in history. No new correction or review work can be admitted.',
      ),
      blockers: ['The Task is cancelled.'],
      escalatedToHuman: false,
    }
  }

  if (overview.revoked_at || isAuthorizationStale(overview)) {
    return {
      action: action(
        'authorize',
        'blocked',
        'You',
        overview.revoked_at
          ? 'Coordination authorization is revoked.'
          : 'The saved coordination team changed.',
        'The report and triage are preserved. Review the current team and authorization before admitting more automatic work.',
      ),
      blockers: [
        overview.revoked_at
          ? 'Coordination authorization is revoked.'
          : 'The saved Task team no longer matches the authorized snapshot.',
      ],
      escalatedToHuman: false,
    }
  }

  const incompleteSource =
    !request.review_report_artifact_id ||
    !request.review_report_digest ||
    !request.review_producer_execution_id
  if (incompleteSource) {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The review-findings request has incomplete source references.',
        'Forge must identify the exact ReviewReport, its digest, and its reviewer Execution before this request can be continued.',
      ),
      blockers: ['The immutable ReviewReport source references are incomplete.'],
      escalatedToHuman: false,
    }
  }

  if (request.state === 'requested' || request.state === 'running') {
    if (overview.project_plan_batch_scope?.batch_state === 'paused') {
      return {
        action: action(
          'blocked',
          'blocked',
          'You',
          'The Project batch is paused.',
          'The exact coordinator request may finish. Resume this Project batch before automatic correction or review work can continue.',
        ),
        blockers: ['The Project batch is paused.'],
        escalatedToHuman: false,
      }
    }
    if (overview.dispatch.state === 'paused') {
      return {
        action: action(
          'resume',
          'blocked',
          'You',
          'The Task is paused while review findings are being triaged.',
          'An admitted read-only coordinator turn may finish. Follow-up work remains paused until the Task is resumed.',
        ),
        blockers: ['The Task dispatch is paused.'],
        escalatedToHuman: false,
      }
    }
    return {
      action: action(
        'wait',
        'wait',
        'Coordinator Agent',
        'The coordinator is evaluating this exact ReviewReport.',
        'The durable review-findings request is still in progress. No disposition, correction, or Gate result is inferred yet.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  if (request.state === 'uncertain') {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The coordinator start outcome is uncertain.',
        'Inspect this exact request and its provider state. Do not start a duplicate coordinator turn from the generic plan action.',
      ),
      blockers: ['The review-findings provider start has an uncertain outcome.'],
      escalatedToHuman: false,
    }
  }

  if (request.state === 'failed') {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'Review-findings triage failed.',
        request.failure_message
          ? `Recorded failure: ${request.failure_message}`
          : 'Inspect the recorded failure and its blocker before deciding how to continue.',
      ),
      blockers: ['The review-findings request failed.'],
      escalatedToHuman: false,
    }
  }

  if (request.state === 'cancelled') {
    return {
      action: action(
        'manual',
        'info',
        'You',
        'The review-findings request was cancelled.',
        'The source report remains available. No correction or fresh review is implied by this request.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  if (request.state !== 'triaged') {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The review-findings request has an unexpected state.',
        `Recorded state: ${request.state}. Inspect the exact request before continuing.`,
      ),
      blockers: [`Unexpected review-findings request state: ${request.state}.`],
      escalatedToHuman: false,
    }
  }

  if (!triage) {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The request is marked triaged, but its matching decision receipt is missing.',
        'The request and triage must refer to the same ReviewReport and coordinator Execution. Refresh the Task records before admitting follow-up work.',
      ),
      blockers: ['No matching immutable review triage receipt is visible.'],
      escalatedToHuman: false,
    }
  }

  if (triage.disposition === 'human') {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The coordinator requested a human decision.',
        'No correction or fresh review WorkUnit is created by this disposition. Review the exact report and recorded rationale before deciding how to continue.',
      ),
      blockers: ['The coordinator disposition requires a human decision.'],
      escalatedToHuman: true,
    }
  }

  if (overview.project_plan_batch_scope?.batch_state === 'paused') {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The Project batch is paused after review findings were triaged.',
        'The exact disposition is recorded. Resume this Project batch before automatic correction or review work can continue.',
      ),
      blockers: ['The Project batch is paused.'],
      escalatedToHuman: false,
    }
  }

  if (overview.dispatch.state === 'paused') {
    return {
      action: action(
        'resume',
        'blocked',
        'You',
        'The Task is paused after review findings were triaged.',
        'The disposition is recorded. Resume the Task before automatic correction or review work can continue.',
      ),
      blockers: ['The Task dispatch is paused.'],
      escalatedToHuman: false,
    }
  }

  if (triage.disposition === 'recheck') {
    const reviewWorkUnitId = triage.review_work_unit_id
    if (!reviewWorkUnitId) {
      return {
        action: action(
          'blocked',
          'blocked',
          'You',
          'The recheck decision has no recorded review WorkUnit.',
          'Inspect the exact triage receipt; do not treat the disposition itself as a new review result.',
        ),
        blockers: ['The recheck triage receipt has no review WorkUnit reference.'],
        escalatedToHuman: false,
      }
    }

    const unavailableWorkUnits = unavailableContinuationData(
      workUnitsState,
      'the exact review WorkUnit',
    )
    if (unavailableWorkUnits) return unavailableWorkUnits

    const reviewWorkUnit = workUnits.find((unit) => unit.id === reviewWorkUnitId) ?? null
    if (!reviewWorkUnit) {
      return {
        action: action(
          'blocked',
          'blocked',
          'You',
          'A fresh review WorkUnit was recorded but is missing from the current Task view.',
          'Refresh Work and inspect the linked review WorkUnit. The recheck disposition does not pass the review line or Gate.',
        ),
        blockers: ['The linked review WorkUnit is not in the current WorkUnit projection.'],
        escalatedToHuman: false,
      }
    }

    if (reviewWorkUnit.status === 'cancelled') {
      return {
        action: action(
          'blocked',
          'blocked',
          'You',
          'The recheck WorkUnit was cancelled.',
          'The original ReviewReport remains unchanged. Decide whether an authorized fresh review is still needed.',
        ),
        blockers: [`Review WorkUnit ${reviewWorkUnitId} was cancelled.`],
        escalatedToHuman: true,
      }
    }

    if (reviewWorkUnit.status === 'completed') {
      return {
        action: action(
          'wait',
          'wait',
          'Forge',
          'The fresh review WorkUnit is complete.',
          'The current ReviewReport and Gate projections govern the next delivery step. The original report remains immutable.',
        ),
        blockers: [],
        escalatedToHuman: false,
      }
    }

    const hasActiveExecution = reviewWorkUnit.readiness.active_execution_ids.length > 0
    if (!hasActiveExecution && !reviewWorkUnit.readiness.runnable) {
      const dependencies = reviewWorkUnit.readiness.unsatisfied_dependency_ids
      const reason = dependencies.length
        ? `Unsatisfied WorkUnit dependencies: ${dependencies.join(', ')}.`
        : 'The WorkUnit is not currently runnable.'
      return {
        action: action(
          'blocked',
          'blocked',
          'You',
          'The fresh review WorkUnit is not ready for dispatch.',
          `${reason} Inspect its current readiness before expecting a provider start.`,
        ),
        blockers: [reason],
        escalatedToHuman: false,
      }
    }
    const owner = hasActiveExecution
      ? reviewWorkUnit.assigned_actor?.kind === 'human'
        ? 'Assigned reviewer'
        : 'Reviewer Agent'
      : reviewWorkUnit.assigned_actor?.kind === 'human'
        ? 'Assigned reviewer'
        : 'Forge dispatcher'

    return {
      action: action(
        'wait',
        'wait',
        owner,
        hasActiveExecution
          ? 'The fresh review WorkUnit has an active Execution.'
          : 'The fresh review WorkUnit is recorded and awaits dispatch.',
        'The review WorkUnit reassesses the same frozen result. A new ReviewReport is required; this triage is not a PASS.',
      ),
      blockers: reviewWorkUnit.assigned_actor
        ? []
        : ['The fresh review WorkUnit has no assigned Actor in the current Task view.'],
      escalatedToHuman: false,
    }
  }

  const unavailableAutomationIntents = unavailableContinuationData(
    automationIntentsState,
    'the correction continuation receipt',
  )
  if (unavailableAutomationIntents) return unavailableAutomationIntents

  const reworkIntent =
    automationIntents.find(
      (intent) => intent.review_report_artifact_id === triage.review_report_artifact_id,
    ) ?? null
  if (!reworkIntent) {
    return {
      action: action(
        'manual',
        'info',
        'You',
        'Findings were confirmed, but no correction continuation record is linked.',
        'Review the exact correction route in Work. This triage records a decision; it does not itself prove a correction WorkUnit was created.',
      ),
      blockers: ['No correction automation intent is linked to this ReviewReport.'],
      escalatedToHuman: false,
    }
  }

  if (reworkIntent.state === 'pending') {
    return {
      action: action(
        'wait',
        'wait',
        'Forge',
        'Automatic correction is durably queued.',
        'Forge will create the correction and next review after the Task resumes and the saved authority is revalidated. No WorkUnit is claimed before the receipt says it exists.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  if (reworkIntent.state === 'blocked') {
    const reason = reworkIntent.block_reason || 'the saved correction route is no longer eligible'
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'Automatic correction needs a human decision.',
        `Recorded blocker: ${reason}`,
      ),
      blockers: [`Automatic correction is blocked: ${reason}`],
      escalatedToHuman: true,
    }
  }

  if (reworkIntent.state === 'cancelled') {
    return {
      action: action(
        'manual',
        'info',
        'You',
        'Automatic correction was cancelled.',
        'No new WorkUnits were created by this route. Review the exact report and decide whether an authorized continuation is needed.',
      ),
      blockers: ['Automatic correction was cancelled.'],
      escalatedToHuman: false,
    }
  }

  const unavailableWorkUnits = unavailableContinuationData(
    workUnitsState,
    'the exact correction and review WorkUnits',
  )
  if (unavailableWorkUnits) return unavailableWorkUnits

  const correctionWorkUnit = reworkIntent.correction_work_unit_id
    ? workUnits.find((unit) => unit.id === reworkIntent.correction_work_unit_id) ?? null
    : null
  const reviewWorkUnit = reworkIntent.review_work_unit_id
    ? workUnits.find((unit) => unit.id === reworkIntent.review_work_unit_id) ?? null
    : null
  if (!correctionWorkUnit || !reviewWorkUnit) {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'The correction receipt is complete, but its WorkUnits are missing from the current Task view.',
        'Refresh Work and inspect the exact correction and review WorkUnits recorded by the receipt.',
      ),
      blockers: ['The correction receipt references WorkUnits missing from the current Task projection.'],
      escalatedToHuman: false,
    }
  }

  if (correctionWorkUnit.status === 'cancelled' || reviewWorkUnit.status === 'cancelled') {
    return {
      action: action(
        'blocked',
        'blocked',
        'You',
        'A WorkUnit in the recorded correction round was cancelled.',
        'The exact automation receipt and ReviewReport remain in history. Decide whether a new authorized route is needed.',
      ),
      blockers: ['A correction or review WorkUnit in the recorded round was cancelled.'],
      escalatedToHuman: true,
    }
  }

  const correctionRunning = correctionWorkUnit.readiness.active_execution_ids.length > 0
  if (correctionWorkUnit.status !== 'completed' || correctionRunning) {
    return {
      action: action(
        'wait',
        'wait',
        correctionWorkUnit.assigned_actor ? 'Assigned implementer' : 'Forge dispatcher',
        correctionRunning
          ? 'The exact correction WorkUnit has an active Execution.'
          : 'The exact correction WorkUnit is recorded and remains open.',
        'The correction still needs a fresh ReviewReport before any Gate decision.',
      ),
      blockers: correctionWorkUnit.assigned_actor
        ? []
        : ['The correction WorkUnit has no assigned Actor in the current Task view.'],
      escalatedToHuman: false,
    }
  }

  if (reviewWorkUnit.status === 'completed') {
    return {
      action: action(
        'wait',
        'wait',
        'Forge',
        'The correction and its review WorkUnits are complete.',
        'The current ReviewReport, review-line, and Gate projections govern the next delivery step. The coordinator triage does not pass a Gate.',
      ),
      blockers: [],
      escalatedToHuman: false,
    }
  }

  const reviewRunning = reviewWorkUnit.readiness.active_execution_ids.length > 0
  return {
    action: action(
      'wait',
      'wait',
      reviewWorkUnit.assigned_actor ? 'Assigned reviewer' : 'Forge dispatcher',
      reviewRunning
        ? 'The fresh reviewer WorkUnit has an active Execution.'
        : 'The correction is complete; the fresh reviewer WorkUnit remains open.',
      `Review WorkUnit ${reviewWorkUnit.id} must produce a new ReviewReport for the corrected result. The original ReviewReport remains historical evidence.`,
    ),
    blockers: reviewWorkUnit.assigned_actor
      ? []
      : ['The fresh reviewer WorkUnit has no assigned Actor in the current Task view.'],
    escalatedToHuman: false,
  }
}

export function deriveReviewFindingsProjection({
  overview,
  request,
  automationIntents = [],
  automationIntentsState = 'loaded',
  workUnits = [],
  workUnitsState = 'loaded',
}: ReviewFindingsProjectionInput): ReviewFindingsProjection | null {
  if (!request) return null
  if (request.kind !== 'review_findings') return null

  const triage = exactTriage(request, overview)
  const projected = projectReviewFindingsAction(
    overview,
    request,
    triage,
    automationIntents,
    automationIntentsState,
    workUnits,
    workUnitsState,
  )

  return {
    request,
    reportId: request.review_report_artifact_id,
    reportDigest: request.review_report_digest,
    producerExecutionId: request.review_producer_execution_id,
    stateLabel: readableRequestState(request.state),
    triage,
    reworkIntent:
      triage?.disposition === 'correct'
        ? automationIntents.find(
            (intent) => intent.review_report_artifact_id === triage.review_report_artifact_id,
          ) ?? null
        : null,
    reviewWorkUnit:
      triage?.disposition === 'recheck' && triage.review_work_unit_id
        ? workUnits.find((unit) => unit.id === triage.review_work_unit_id) ?? null
        : null,
    correctionWorkUnit:
      triage?.disposition === 'correct'
        ? (() => {
            const intent = automationIntents.find(
              (candidate) => candidate.review_report_artifact_id === triage.review_report_artifact_id,
            )
            return intent?.correction_work_unit_id
              ? workUnits.find((unit) => unit.id === intent.correction_work_unit_id) ?? null
              : null
          })()
        : null,
    action: projected.action,
    blockers: projected.blockers,
    escalatedToHuman: projected.escalatedToHuman,
  }
}
