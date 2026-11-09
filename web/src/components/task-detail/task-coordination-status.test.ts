import { describe, expect, it } from 'vitest'
import type {
  TaskCoordinationOverviewResponse,
  TaskCoordinationPlanResponse,
  TaskCoordinationRequestResponse,
} from '@/types/generated'
import {
  deriveCoordinationNextAction,
  isQualityBudgetExhausted,
  isTurnBudgetExhausted,
  readableCoordinationCode,
} from './task-coordination-status'

function overview(
  overrides: Partial<TaskCoordinationOverviewResponse> = {},
): TaskCoordinationOverviewResponse {
  return {
    task_id: 'task-1',
    task_version: 4,
    lifecycle: { task_id: 'task-1', state: 'active', version: 2, reason_kind: null, reason_ref: null, created_at: '', updated_at: '' },
    dispatch: { task_id: 'task-1', state: 'idle', version: 1, requested_by: null, reason: null, created_at: '', updated_at: '' },
    mode: 'proposal',
    authorization_version: 2,
    revoked_at: null,
    revoke_reason: null,
    requested_by: 'user-1',
    coordinator: {
      task_role_id: 'role-1',
      role: 'orchestrator',
      actor_ref: { kind: 'agent', id: 'agent-1' },
      display_name: 'Coordinator',
      harness: 'codex',
    },
    team: [
      {
        id: 'role-1',
        role: 'orchestrator',
        display_label: 'Coordinator',
        coordination_mode: 'collaborative',
        capabilities: ['orchestration'],
        configuration: { schema_version: 1, display_label: 'Coordinator', capabilities: ['orchestration'], rework: null },
        version: 1,
        members: [
          {
            id: 'member-1',
            actor_ref: { kind: 'agent', id: 'agent-1' },
            display_name: 'Coordinator',
            harness: 'codex',
            status: 'active',
            version: 1,
          },
        ],
      },
    ],
    team_digest: 'a'.repeat(64),
    envelope: {},
    envelope_digest: 'b'.repeat(64),
    turn_count: 1,
    max_turns: 8,
    quality_request_count: 0,
    max_quality_requests: 8,
    can_authorize: true,
    can_request: true,
    blockers: [],
    latest_request: null,
    latest_plan: null,
    ...overrides,
  }
}

function request(
  overrides: Partial<TaskCoordinationRequestResponse> = {},
): TaskCoordinationRequestResponse {
  return {
    id: 'request-1',
    task_id: 'task-1',
    mode: 'proposal',
    state: 'requested',
    authorization_version: 2,
    objective_digest: 'c'.repeat(64),
    request_digest: 'd'.repeat(64),
    brief: { objective: 'Deliver', constraints: null, acceptance_criteria: [], context: null },
    wake_id: null,
    execution_id: null,
    plan_artifact_id: null,
    failure_code: null,
    failure_message: null,
    version: 1,
    created_at: '',
    updated_at: '',
    completed_at: null,
    ...overrides,
  }
}

function plan(overrides: Partial<TaskCoordinationPlanResponse> = {}): TaskCoordinationPlanResponse {
  return {
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
    ...overrides,
  }
}

describe('coordination next action', () => {
  it('shows a proposed plan as the primary action even though the caller may still reauthorize', () => {
    const next = deriveCoordinationNextAction(overview({ can_authorize: true }), null, plan())
    expect(next.kind).toBe('approve')
    expect(next.owner).toBe('You')
  })

  it('does not offer another turn when the global preparation budget is exhausted', () => {
    const value = overview({ turn_count: 8, can_request: true })
    expect(isTurnBudgetExhausted(value)).toBe(true)
    const next = deriveCoordinationNextAction(value, null, null)
    expect(next.kind).toBe('blocked')
    expect(next.summary).toMatch(/limit/i)
  })

  it('asks a human to reauthorize after revocation without deleting existing work', () => {
    const next = deriveCoordinationNextAction(
      overview({ revoked_at: '2026-11-09T00:00:00Z', can_request: false }),
      null,
      plan({
        state: 'materialized',
        materialization: {
          id: 'mat-1',
          plan_artifact_id: 'artifact-1',
          plan_digest: 'e'.repeat(64),
          work_unit_ids: ['unit-1'],
          receipt_digest: 'f'.repeat(64),
          created_by: 'user-1',
          created_at: '',
        },
      }),
    )
    expect(next.kind).toBe('authorize')
    expect(next.detail).toMatch(/not deleted|preserved/i)
  })

  it('requires reauthorization when the saved team snapshot is stale', () => {
    const next = deriveCoordinationNextAction(
      overview({
        can_authorize: true,
        can_request: true,
        blockers: [
          {
            code: 'coordination.authorization_stale',
            message: 'the Task team changed',
            task_role_id: null,
            actor_ref: null,
          },
        ],
      }),
      null,
      null,
    )
    expect(next.kind).toBe('authorize')
    expect(next.tone).toBe('blocked')
  })

  it('keeps manual planning explicit', () => {
    const next = deriveCoordinationNextAction(overview({ mode: 'manual', can_request: false }), null, null)
    expect(next.kind).toBe('manual')
    expect(next.detail).toMatch(/Advanced work/)
  })

  it('prioritizes resume over materialization when the Task is paused', () => {
    const next = deriveCoordinationNextAction(
      overview({
        dispatch: { task_id: 'task-1', state: 'paused', version: 2, requested_by: null, reason: null, created_at: '', updated_at: '' },
      }),
      null,
      plan({ state: 'approved' }),
    )
    expect(next.kind).toBe('resume')
  })

  it('turns a materialized ready-to-merge Task toward the exact Gate review', () => {
    const next = deriveCoordinationNextAction(
      overview({
        lifecycle: { task_id: 'task-1', state: 'ready_to_merge', version: 4, reason_kind: null, reason_ref: null, created_at: '', updated_at: '' },
      }),
      null,
      plan({
        state: 'materialized',
        materialization: {
          id: 'mat-1',
          plan_artifact_id: 'artifact-1',
          plan_digest: 'e'.repeat(64),
          work_unit_ids: ['unit-1'],
          receipt_digest: 'f'.repeat(64),
          created_by: 'user-1',
          created_at: '',
        },
      }),
    )
    expect(next.summary).toMatch(/Gate review/)
    expect(next.detail).toMatch(/does not run tests/)
  })

  it('does not retry an uncertain provider outcome', () => {
    const next = deriveCoordinationNextAction(
      overview({ can_request: false }),
      request({ state: 'uncertain' }),
      null,
    )
    expect(next.kind).toBe('blocked')
    expect(next.detail).toMatch(/will not repeat/i)
  })

  it('renders readable codes and quality budget state', () => {
    expect(readableCoordinationCode('coordination.paused')).toMatch(/paused/i)
    expect(isQualityBudgetExhausted(overview({ quality_request_count: 8 }))).toBe(true)
  })
})
