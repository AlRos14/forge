import { describe, expect, it } from 'vitest'
import type {
  ReviewLineResponse,
  Task,
  TaskCoordinationOverviewResponse,
  TaskQualityRequestResponse,
  WorkUnitResponse,
} from '@/types/generated'
import { deriveJourney } from './task-journey'

function overview(
  overrides: Partial<TaskCoordinationOverviewResponse> = {},
): TaskCoordinationOverviewResponse {
  return {
    task_id: 'task-1',
    task_version: 4,
    lifecycle: {
      task_id: 'task-1',
      state: 'active',
      version: 2,
      reason_kind: null,
      reason_ref: null,
      created_at: '',
      updated_at: '',
    },
    dispatch: {
      task_id: 'task-1',
      state: 'idle',
      version: 1,
      requested_by: null,
      reason: null,
      created_at: '',
      updated_at: '',
    },
    mode: 'proposal',
    authorization_version: 2,
    revoked_at: null,
    revoke_reason: null,
    requested_by: 'user-1',
    coordinator: null,
    team: [],
    team_digest: 'a'.repeat(64),
    envelope: {},
    envelope_digest: 'b'.repeat(64),
    turn_count: 1,
    max_turns: 8,
    quality_request_count: 0,
    max_quality_requests: 8,
    can_authorize: true,
    can_request: false,
    project_plan_batch_scope: null,
    blockers: [],
    latest_request: null,
    latest_plan: null,
    ...overrides,
  }
}

function task(state: Task['lifecycle']['state'] = 'active'): Task {
  return {
    id: 'task-1',
    project_id: 'project-1',
    repo_id: null,
    parent_task_id: null,
    title: 'Deliver',
    description: null,
    lifecycle: {
      task_id: 'task-1',
      state,
      version: 2,
      reason_kind: null,
      reason_ref: null,
      created_at: '',
      updated_at: '',
    },
    priority: 0,
    board_position: 0,
    subtask_order: null,
    task_roles: [],
    execution_observability: {},
    workspace: null,
    external_issue_number: null,
    external_issue_url: null,
    version: 4,
    created_at: '',
    updated_at: '',
  } as unknown as Task
}

function workUnit(overrides: Partial<WorkUnitResponse> = {}): WorkUnitResponse {
  return {
    id: 'unit-1',
    task_id: 'task-1',
    parent_work_unit_id: null,
    title: 'Implement',
    scope: 'Scope',
    execution_instructions: null,
    status: 'open',
    role: 'implementer',
    assigned_actor: { kind: 'agent', id: 'agent-1' },
    requires_integration: false,
    provenance: null,
    review_approval_gate_id: null,
    review_approval_line_id: null,
    created_by: { kind: 'human', id: 'user-1' },
    version: 1,
    created_at: '',
    updated_at: '',
    review_subject: null,
    readiness: {
      runnable: false,
      ready_for_allocation: false,
      active_execution_ids: [],
      unsatisfied_dependency_ids: [],
      awaiting_integration: false,
      awaiting_acceptance: false,
      awaiting_review_approval: false,
    },
    ...overrides,
  }
}

function quality(overrides: Partial<TaskQualityRequestResponse> = {}): TaskQualityRequestResponse {
  return {
    id: 'quality-1',
    task_id: 'task-1',
    source_kind: 'human',
    source_id: 'user-1',
    category: 'delivery_correction',
    target_kind: 'implementation',
    target_task_role_id: 'role-implementer',
    target_actor: null,
    summary: 'Fix it',
    findings: [],
    resolution_condition: 'Exact review passes',
    state: 'requires_human',
    handoff_id: null,
    work_unit_id: null,
    coordination_request_id: null,
    budget_round: 1,
    version: 1,
    created_at: '',
    updated_at: '',
    resolved_at: null,
    resolution_kind: null,
    resolution_ref: null,
    resolution_digest: null,
    quality_route: {
      id: 'review_line:line-1',
      round: 1,
      limit: 4,
      source_task_role_id: 'role-reviewer',
      escalation: null,
    },
    ...overrides,
  }
}

function reviewLine(overrides: Partial<ReviewLineResponse> = {}): ReviewLineResponse {
  return {
    id: 'line-1',
    task_id: 'task-1',
    root_work_unit_id: 'unit-1',
    current_work_unit_id: 'unit-1',
    current_work_unit_version: 2,
    current_result_execution_id: 'execution-1',
    current_result_artifact_id: 'artifact-result-1',
    current_commit_sha: null,
    current_workspace_id: null,
    current_snapshot_digest: null,
    current_manifest_id: null,
    latest_report_artifact_id: 'artifact-report-1',
    latest_verdict: null,
    revision: 1,
    created_at: '',
    updated_at: '',
    history: [],
    ...overrides,
  }
}

const empty = {
  dispatch: undefined,
  workUnits: [],
  reviewLines: [],
  gates: [],
  qualityRequests: [],
  interventions: [],
}

describe('guided journey projection', () => {
  it('keeps a proposed plan current and asks the human to review it', () => {
    const journey = deriveJourney({
      ...empty,
      task: task(),
      coordination: overview({
        latest_plan: {
          artifact_id: 'artifact-1',
          task_id: 'task-1',
          request_id: 'request-1',
          mode: 'proposal',
          state: 'proposed',
          authorization_version: 2,
          task_version: 4,
          digest: 'e'.repeat(64),
          team_digest: 'a'.repeat(64),
          envelope_digest: 'b'.repeat(64),
          objective_digest: 'c'.repeat(64),
          work_units: [],
          proposal_id: 'proposal-1',
          decision_id: null,
          materialization: null,
          version: 1,
          created_at: '',
          updated_at: '',
        },
      }),
    })
    const plan = journey.stages.find((stage) => stage.id === 'plan')
    expect(plan?.status).toBe('current')
    expect(journey.nextSummary).toMatch(/review/i)
  })

  it('puts an escalated quality cycle ahead of automatic coordination', () => {
    const journey = deriveJourney({
      ...empty,
      task: task(),
      coordination: overview(),
      workUnits: [workUnit()],
      qualityRequests: [quality()],
    })
    expect(journey.escalatedToHuman).toBe(true)
    expect(journey.nextOwner).toBe('You')
    expect(journey.stages.find((stage) => stage.id === 'quality')?.status).toBe('blocked')
    expect(journey.blockers.join(' ')).toMatch(/human/i)
  })

  it('surfaces a per-route cycle escalation reason from the durable route contract', () => {
    const journey = deriveJourney({
      ...empty,
      task: task(),
      coordination: overview(),
      workUnits: [workUnit()],
      qualityRequests: [
        quality({
          quality_route: {
            id: 'review_line:line-1',
            round: 3,
            limit: 4,
            source_task_role_id: 'role-reviewer',
            escalation: {
              reason_code: 'coordination.quality_route_ping_pong',
              reason:
                'pending quality targets alternate between TaskRoles reviewer and implementer',
              cycle_request_ids: ['q1', 'q2', 'q3'],
              cycle_task_role_ids: ['role-a', 'role-b', 'role-a'],
            },
          },
        }),
      ],
    })
    const stage = journey.stages.find((candidate) => candidate.id === 'quality')
    expect(stage?.status).toBe('blocked')
    expect(stage?.summary).toMatch(/Quality route escalated/)
    expect(stage?.detail).toMatch(/alternate between TaskRoles/)
    expect(journey.blockers.join(' ')).toMatch(/Quality route escalated/)
  })

  it('blocks execution when dispatch is paused but keeps existing work visible', () => {
    const journey = deriveJourney({
      ...empty,
      task: task(),
      coordination: overview({
        dispatch: {
          task_id: 'task-1',
          state: 'paused',
          version: 2,
          requested_by: null,
          reason: null,
          created_at: '',
          updated_at: '',
        },
      }),
      workUnits: [workUnit()],
    })
    const execution = journey.stages.find((stage) => stage.id === 'execution')
    expect(execution?.status).toBe('blocked')
    expect(execution?.summary).toMatch(/paused/i)
  })

  it('shows a correction round while the exact result waits for an independent review', () => {
    const journey = deriveJourney({
      ...empty,
      task: task(),
      coordination: overview(),
      workUnits: [workUnit({ status: 'completed' })],
      reviewLines: [reviewLine({ revision: 2, latest_verdict: 'request_changes' })],
    })
    const quality = journey.stages.find((stage) => stage.id === 'quality')
    expect(quality?.status).toBe('current')
    expect(quality?.summary).toMatch(/Review revision 2/)
  })

  it('treats a passing ReviewReport as quality complete even without a repository Artifact', () => {
    const journey = deriveJourney({
      ...empty,
      task: task(),
      coordination: overview(),
      workUnits: [workUnit({ status: 'completed' })],
      reviewLines: [reviewLine({ revision: 2, latest_verdict: 'pass' })],
    })
    expect(journey.stages.find((stage) => stage.id === 'quality')?.status).toBe('done')
  })

  it('marks delivery complete only on the aggregate TaskLifecycle', () => {
    const journey = deriveJourney({
      ...empty,
      task: task('done'),
      coordination: overview({
        lifecycle: {
          ...overview().lifecycle,
          state: 'done',
        },
        dispatch: {
          ...overview().dispatch,
          state: 'cancelled',
        },
      }),
      qualityRequests: [quality()],
    })
    expect(journey.stages.find((stage) => stage.id === 'delivery')?.status).toBe('done')
    expect(journey.nextOwner).toBe('Nobody')
    expect(journey.escalatedToHuman).toBe(false)
    expect(journey.nextSummary).toMatch(/Task is done/i)
  })

  it('keeps ready-to-merge lifecycle ahead of stale quality escalation', () => {
    const journey = deriveJourney({
      ...empty,
      task: task('ready_to_merge'),
      coordination: overview({
        lifecycle: {
          ...overview().lifecycle,
          state: 'ready_to_merge',
        },
      }),
      qualityRequests: [quality()],
    })

    expect(journey.escalatedToHuman).toBe(false)
    expect(journey.nextSummary).toMatch(/ready for integration/i)
    expect(journey.stages.find((stage) => stage.id === 'delivery')?.summary).toMatch(
      /Ready for integration/,
    )
  })
})
