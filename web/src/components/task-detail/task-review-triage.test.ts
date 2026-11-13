import { describe, expect, it } from 'vitest'
import type {
  TaskCoordinationOverviewResponse,
  TaskCoordinationRequestResponse,
  TaskReviewTriageResponse,
  TaskReworkAutomationIntentResponse,
  WorkUnitResponse,
} from '@/types/generated'
import { deriveReviewFindingsProjection } from './task-review-triage'

function overview(
  {
    triages = [],
    dispatchState = 'idle',
    lifecycleState = 'active',
    revokedAt = null,
    batchState = null,
  }: {
    triages?: TaskReviewTriageResponse[]
    dispatchState?: 'idle' | 'paused' | 'cancelled'
    lifecycleState?: TaskCoordinationOverviewResponse['lifecycle']['state']
    revokedAt?: string | null
    batchState?: string | null
  } = {},
): TaskCoordinationOverviewResponse {
  return {
    task_id: 'task-1',
    task_version: 4,
    lifecycle: {
      task_id: 'task-1',
      state: lifecycleState,
      version: 2,
      reason_kind: null,
      reason_ref: null,
      created_at: '',
      updated_at: '',
    },
    dispatch: {
      task_id: 'task-1',
      state: dispatchState,
      version: 1,
      requested_by: null,
      reason: null,
      created_at: '',
      updated_at: '',
    },
    mode: 'prepare_and_execute',
    authorization_version: 3,
    revoked_at: revokedAt,
    revoke_reason: null,
    requested_by: 'user-1',
    coordinator: null,
    team: [],
    team_digest: 'team-digest',
    envelope: {},
    envelope_digest: 'envelope-digest',
    turn_count: 2,
    max_turns: 8,
    quality_request_count: 0,
    max_quality_requests: 8,
    can_authorize: true,
    can_request: true,
    project_plan_batch_scope: batchState
      ? { batch_id: 'batch-1', batch_state: batchState, task_state: 'started', blocker: null }
      : null,
    blockers: [],
    latest_request: null,
    latest_plan: null,
    review_triage: triages,
  }
}

function request(
  overrides: Partial<{
    state: 'requested' | 'running' | 'triaged' | 'uncertain' | 'failed' | 'cancelled'
    execution_id: string | null
    review_report_artifact_id: string | null
    review_report_digest: string | null
    review_producer_execution_id: string | null
  }> = {},
): TaskCoordinationRequestResponse {
  return {
    id: 'coordination-request-1',
    task_id: 'task-1',
    kind: 'review_findings',
    mode: 'prepare_and_execute',
    state: overrides.state ?? 'requested',
    authorization_version: 3,
    objective_digest: 'objective-digest',
    request_digest: 'request-digest',
    brief: { objective: 'Review exact findings', constraints: null, acceptance_criteria: [], context: null },
    wake_id: 'wake-1',
    execution_id: overrides.execution_id ?? null,
    plan_artifact_id: null,
    failure_code: null,
    failure_message: null,
    version: 1,
    created_at: '',
    updated_at: '',
    completed_at: null,
    review_report_artifact_id:
      overrides.review_report_artifact_id === undefined
        ? 'review-report-1'
        : overrides.review_report_artifact_id,
    review_report_digest:
      overrides.review_report_digest === undefined
        ? 'a'.repeat(64)
        : overrides.review_report_digest,
    review_producer_execution_id:
      overrides.review_producer_execution_id === undefined
        ? 'reviewer-execution-1'
        : overrides.review_producer_execution_id,
  } satisfies TaskCoordinationRequestResponse
}

function triage(
  disposition: TaskReviewTriageResponse['disposition'],
  overrides: Partial<TaskReviewTriageResponse> = {},
): TaskReviewTriageResponse {
  return {
    review_report_artifact_id: 'review-report-1',
    execution_id: 'coordinator-execution-1',
    disposition,
    rationale: 'The report finding is reproducible.',
    instructions: 'Stay within the existing Task scope.',
    review_work_unit_id: disposition === 'recheck' ? 'review-work-unit-1' : null,
    created_at: '',
    ...overrides,
  }
}

function workUnit(
  id: string,
  overrides: Partial<WorkUnitResponse> = {},
): WorkUnitResponse {
  return {
    id,
    task_id: 'task-1',
    parent_work_unit_id: null,
    title: 'Recheck exact result',
    scope: 'Same frozen result',
    execution_instructions: null,
    status: 'open',
    role: 'reviewer',
    assigned_actor: { kind: 'agent', id: 'reviewer-1' },
    requires_integration: false,
    provenance: null,
    review_approval_gate_id: null,
    review_approval_line_id: null,
    created_by: { kind: 'agent', id: 'coordinator-1' },
    version: 1,
    created_at: '',
    updated_at: '',
    readiness: {
      runnable: true,
      ready_for_allocation: true,
      active_execution_ids: [],
      unsatisfied_dependency_ids: [],
      awaiting_integration: false,
      awaiting_acceptance: false,
      awaiting_review_approval: false,
    },
    review_subject: null,
    ...overrides,
  }
}

function automationIntent(
  state: TaskReworkAutomationIntentResponse['state'],
  overrides: Partial<TaskReworkAutomationIntentResponse> = {},
): TaskReworkAutomationIntentResponse {
  return {
    id: 'automation-1',
    review_report_artifact_id: 'review-report-1',
    reviewed_work_unit_id: 'implementation-1',
    reviewed_work_unit_version: 2,
    state,
    block_reason: null,
    correction_work_unit_id: null,
    review_work_unit_id: null,
    reconciled_manual_receipt_id: null,
    rework_round: null,
    created_at: '',
    updated_at: '',
    ...overrides,
  }
}

describe('review findings continuation projection', () => {
  it('shows an in-flight exact report request and keeps reviewer and coordinator executions distinct', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview(),
      request: request({ state: 'running', execution_id: 'coordinator-execution-1' }),
    })

    expect(projection?.stateLabel).toBe('running')
    expect(projection?.reportId).toBe('review-report-1')
    expect(projection?.producerExecutionId).toBe('reviewer-execution-1')
    expect(projection?.action.owner).toBe('Coordinator Agent')
    expect(projection?.action.detail).toMatch(/No disposition, correction, or Gate result is inferred/)
  })

  it('fails closed when the triaged request does not match the coordinator execution receipt', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')] }),
      request: request({ state: 'triaged', execution_id: 'different-coordinator-execution' }),
    })

    expect(projection?.triage).toBeNull()
    expect(projection?.action.kind).toBe('blocked')
    expect(projection?.action.summary).toMatch(/receipt is missing/i)
  })

  it('fails closed when nullable ReviewReport source fields are absent', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview(),
      request: request({
        state: 'triaged',
        execution_id: 'coordinator-execution-1',
        review_report_digest: null,
      }),
    })

    expect(projection?.reportDigest).toBeNull()
    expect(projection?.action.kind).toBe('blocked')
    expect(projection?.action.summary).toMatch(/incomplete source references/i)
  })

  it('does not treat a correct disposition as a created correction without a continuation intent', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
    })

    expect(projection?.triage?.disposition).toBe('correct')
    expect(projection?.action.kind).toBe('manual')
    expect(projection?.action.detail).toMatch(/does not itself prove a correction WorkUnit was created/i)
  })

  it('shows queued correction only from the exact durable automation intent', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      automationIntents: [automationIntent('pending')],
    })

    expect(projection?.action.owner).toBe('Forge')
    expect(projection?.action.summary).toMatch(/durably queued/i)
    expect(projection?.action.detail).toMatch(/does not prove/i)
  })

  it('keeps a human disposition blocked and does not create correction or review work', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('human')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      automationIntents: [automationIntent('pending')],
    })

    expect(projection?.escalatedToHuman).toBe(true)
    expect(projection?.action.kind).toBe('blocked')
    expect(projection?.action.owner).toBe('You')
    expect(projection?.action.detail).toMatch(/No correction or fresh review WorkUnit is created/)
  })

  it('shows the exact recheck WorkUnit as awaiting dispatch without calling it a PASS', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('recheck')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      workUnits: [workUnit('review-work-unit-1')],
    })

    expect(projection?.reviewWorkUnit?.id).toBe('review-work-unit-1')
    expect(projection?.action.summary).toMatch(/awaits dispatch/i)
    expect(projection?.action.detail).toMatch(/This triage is not a PASS/)
  })

  it('does not offer a duplicate request after an uncertain provider start', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview(),
      request: request({ state: 'uncertain' }),
    })

    expect(projection?.action.kind).toBe('blocked')
    expect(projection?.action.detail).toMatch(/Do not start a duplicate/i)
  })

  it('blocks an exact recheck follow-up while Task dispatch is paused', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('recheck')], dispatchState: 'paused' }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      workUnits: [workUnit('review-work-unit-1')],
    })

    expect(projection?.action.kind).toBe('resume')
    expect(projection?.action.summary).toMatch(/paused after review findings/i)
    expect(projection?.blockers).toContain('The Task dispatch is paused.')
  })

  it('blocks a correction follow-up while its Project batch is paused', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')], batchState: 'paused' }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      automationIntents: [automationIntent('pending')],
    })

    expect(projection?.action.kind).toBe('blocked')
    expect(projection?.action.summary).toMatch(/Project batch is paused/i)
    expect(projection?.blockers).toContain('The Project batch is paused.')
  })

  it('blocks a triaged follow-up after coordination authorization is revoked', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')], revokedAt: 'revoked-at' }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      automationIntents: [automationIntent('pending')],
    })

    expect(projection?.action.kind).toBe('authorize')
    expect(projection?.action.summary).toMatch(/authorization is revoked/i)
    expect(projection?.blockers).toContain('Coordination authorization is revoked.')
  })

  it('keeps TaskLifecycle done ahead of a stale triage or dispatch state', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({
        triages: [triage('human')],
        lifecycleState: 'done',
        dispatchState: 'cancelled',
        revokedAt: 'revoked-at',
      }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
    })

    expect(projection?.action.kind).toBe('done')
    expect(projection?.escalatedToHuman).toBe(false)
    expect(projection?.blockers).toEqual([])
  })

  it('keeps ready-to-merge lifecycle authority ahead of paused batch triage follow-up', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({
        triages: [triage('human')],
        lifecycleState: 'ready_to_merge',
        batchState: 'paused',
      }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
    })

    expect(projection?.action.summary).toMatch(/ready for integration/i)
    expect(projection?.action.detail).toMatch(/does not alter that authority/i)
    expect(projection?.escalatedToHuman).toBe(false)
    expect(projection?.blockers).toEqual([])
  })

  it('waits for continuation receipt loading instead of reporting no receipt', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      automationIntentsState: 'loading',
    })

    expect(projection?.action.kind).toBe('wait')
    expect(projection?.action.summary).toMatch(/loading the correction continuation receipt/i)
    expect(projection?.blockers).toEqual([])
  })

  it('surfaces a retryable evidence error instead of inferring a missing WorkUnit', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('recheck')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      workUnitsState: 'error',
    })

    expect(projection?.action.kind).toBe('blocked')
    expect(projection?.action.summary).toMatch(/could not load the exact review WorkUnit/i)
    expect(projection?.action.detail).toMatch(/retry loading/i)
  })

  it('lets completed correction and review WorkUnits flow to current Gate evidence', () => {
    const projection = deriveReviewFindingsProjection({
      overview: overview({ triages: [triage('correct')] }),
      request: request({ state: 'triaged', execution_id: 'coordinator-execution-1' }),
      automationIntents: [
        automationIntent('completed', {
          correction_work_unit_id: 'correction-work-unit-1',
          review_work_unit_id: 'fresh-review-work-unit-1',
        }),
      ],
      workUnits: [
        workUnit('correction-work-unit-1', { status: 'completed' }),
        workUnit('fresh-review-work-unit-1', { status: 'completed' }),
      ],
    })

    expect(projection?.action.kind).toBe('wait')
    expect(projection?.action.summary).toMatch(/WorkUnits are complete/i)
    expect(projection?.action.detail).toMatch(/triage does not pass a Gate/i)
  })
})
