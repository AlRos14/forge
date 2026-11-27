import { describe, expect, it, vi } from 'vitest'
import type { QueryClient } from '@tanstack/react-query'
import type { ForgeEvent, PublicEventType } from '@/types/generated'
import { routeSsePayload } from './sse'

function event(event_type: string, fields: Partial<ForgeEvent> = {}): ForgeEvent {
  return {
    event_type: event_type as PublicEventType,
    entity_id: 'entity-1',
    timestamp: '2026-10-10T00:00:00Z',
    event_id: null,
    sequence: null,
    entity_type: null,
    scope_type: null,
    scope_id: null,
    payload: null,
    ...fields,
  }
}

function createMocks() {
  const invalidateQueries = vi.fn()
  const queryClient = { invalidateQueries } as unknown as QueryClient
  const dispatch = vi.fn()
  return { queryClient, invalidateQueries, dispatch }
}

describe('routeSsePayload', () => {
  it('invalidates Task and Gate projections for a durable lifecycle event', () => {
    const { queryClient, invalidateQueries, dispatch } = createMocks()
    routeSsePayload(
      event('task.lifecycle_changed', {
        entity_id: 'task-1',
        entity_type: 'task',
        scope_type: 'task',
        scope_id: 'task-1',
        payload: { task_id: 'task-1', to_state: 'active' },
      }),
      queryClient,
      { dispatch },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['tasks', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'detail'] }],
        [{ queryKey: ['task-dispatch', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'gates'] }],
      ]),
    )
    expect(dispatch).not.toHaveBeenCalled()
  })

  it('invalidates exact Task-scoped execution projections', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('execution.completed', {
        entity_id: 'execution-1',
        entity_type: 'execution',
        scope_type: 'task',
        scope_id: 'task-1',
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['executions', 'execution-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'executions'] }],
        [{ queryKey: ['tasks', 'task-1', 'work-units'] }],
        [{ queryKey: ['task-dispatch', 'task-1'] }],
        [{ queryKey: ['tasks', 'task-1', 'diff'] }],
      ]),
    )
  })

  it('invalidates Project detail and list after an authorized deletion hint', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('project.deleted', {
        entity_id: 'project-1',
        scope_type: 'user',
        scope_id: 'member-b',
        payload: {},
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['projects', 'project-1'] }],
        [{ queryKey: ['projects'] }],
      ]),
    )
  })

  it('invalidates the Project DAG projection for named plan-change events', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('project.plan_changed', {
        entity_id: 'batch-1',
        entity_type: 'project_plan_batch',
        scope_type: 'project',
        scope_id: 'project-1',
        payload: { project_id: 'project-1', batch_id: 'batch-1', version: 3, state: 'running' },
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries.mock.calls).toEqual(
      expect.arrayContaining([
        [{ queryKey: ['projects', 'project-1', 'plan'] }],
        [{ queryKey: ['projects', 'project-1', 'tasks'] }],
        [{ queryKey: ['projects', 'project-1'] }],
      ]),
    )
  })

  it('keeps retired status, Agent Chat, and legacy Review events out of UI projections', () => {
    for (const event_type of [
      'task.status_changed',
      'agent_chat.message_created',
      'review.passed',
    ]) {
      const { queryClient, invalidateQueries, dispatch } = createMocks()
      routeSsePayload(event(event_type, { entity_id: 'legacy-1' }), queryClient, { dispatch })
      expect(invalidateQueries).not.toHaveBeenCalled()
      expect(dispatch).not.toHaveBeenCalled()
    }
  })

  it('invalidates the review coverage query when review coverage changes', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('review.coverage_changed', { payload: { task_id: 'task-1' } }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: ['tasks', 'task-1', 'review-coverage'],
    })
  })

  it('refreshes coordination, quality, and intervention projections from Task events', () => {
    const cases: Array<[string, string[]]> = [
      ['task.review_triaged', ['tasks::task-1::coordination', 'tasks::task-1::work-units']],
      [
        'task.autonomy_limits_changed',
        [
          'task-autonomy-limits::task-1',
          'tasks::task-1::coordination',
          'tasks::task-1::quality-requests',
        ],
      ],
      [
        'task.coordination_plan_proposed',
        [
          'tasks::task-1::coordination',
          'tasks::task-1',
          'task-dispatch::task-1',
          'tasks::task-1::work-units',
        ],
      ],
      [
        'task.repository_set_replanned',
        [
          'tasks::task-1::coordination',
          'tasks::task-1',
          'task-dispatch::task-1',
          'tasks::task-1::work-units',
        ],
      ],
      [
        'task.quality_requested',
        [
          'tasks::task-1::quality-requests',
          'tasks::task-1::coordination',
          'tasks::task-1::work-units',
        ],
      ],
      [
        'task.intervention_recorded',
        ['tasks::task-1::interventions', 'tasks::task-1::coordination', 'task-dispatch::task-1'],
      ],
    ]
    for (const [eventType, expected] of cases) {
      const { queryClient, invalidateQueries } = createMocks()
      routeSsePayload(
        event(eventType, {
          entity_id: 'entity-1',
          scope_type: 'task',
          scope_id: 'task-1',
          payload: { task_id: 'task-1' },
        }),
        queryClient,
        { dispatch: vi.fn() },
      )
      const serialized = invalidateQueries.mock.calls.map(([call]) =>
        JSON.stringify((call as { queryKey: unknown[] }).queryKey),
      )
      for (const suffix of expected) {
        expect(serialized, `${eventType} should refresh ${suffix}`).toContain(
          JSON.stringify(suffix.split('::')),
        )
      }
    }
  })

  it('refreshes Project DAG projections when an autonomy change can resume a batch Task', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('task.autonomy_limits_changed', {
        entity_id: 'task-1',
        scope_type: 'task',
        scope_id: 'task-1',
        payload: { task_id: 'task-1' },
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    const projectPlanPredicate = invalidateQueries.mock.calls
      .map(([filters]) => filters?.predicate)
      .find(
        (predicate) =>
          typeof predicate === 'function' &&
          predicate({ queryKey: ['projects', 'project-1', 'plan'] } as never),
      )

    expect(projectPlanPredicate).toBeDefined()
    expect(projectPlanPredicate?.({ queryKey: ['projects', 'project-1', 'tasks'] } as never)).toBe(
      false,
    )
  })

  it('resynchronizes active queries when the public stream reports lag', () => {
    const { queryClient, invalidateQueries } = createMocks()
    routeSsePayload(
      event('events.resync_required', {
        entity_id: 'events.resync_required',
      }),
      queryClient,
      { dispatch: vi.fn() },
    )
    expect(invalidateQueries).toHaveBeenCalledWith(
      expect.objectContaining({ refetchType: 'active' }),
    )
  })

  it('invalidates verification and decision queries for the new public events', () => {
    for (const name of [
      'task.validation_check_requested',
      'task.validation_check_completed',
      'task.validation_comparison_recorded',
      'task.human_decision_requested',
      'task.human_decision_answered',
      'task.human_decision_closed',
    ]) {
      const { queryClient, invalidateQueries, dispatch } = createMocks()
      routeSsePayload(
        event(name, { scope_type: 'task', scope_id: 'task-1', payload: { task_id: 'task-1' } }),
        queryClient,
        { dispatch },
      )
      expect(invalidateQueries.mock.calls).toEqual(
        expect.arrayContaining([
          [{ queryKey: ['tasks', 'task-1', 'human-decisions'] }],
          [{ queryKey: ['tasks', 'task-1', 'validation-check-requests'] }],
          [{ queryKey: ['tasks', 'task-1', 'validation-comparisons'] }],
        ]),
      )
    }
  })

  it('invalidates PR feedback, rework, action, and Task decision projections for PR events', () => {
    for (const name of [
      'task.pr_feedback_ingested',
      'task.pr_feedback_triaged',
      'task.pr_rework_superseded',
      'task.pr_rework_resumed',
      'task.pr_rework_converged',
      'task.pr_followup_proposed',
      'task.pr_action_requested',
      'task.pr_action_completed',
    ]) {
      const { queryClient, invalidateQueries } = createMocks()
      routeSsePayload(
        event(name, { scope_type: 'task', scope_id: 'task-1', payload: { task_id: 'task-1' } }),
        queryClient,
        { dispatch: vi.fn() },
      )
      expect(invalidateQueries.mock.calls).toEqual(
        expect.arrayContaining([
          [{ queryKey: ['tasks', 'task-1', 'pr-feedback'] }],
          [{ queryKey: ['tasks', 'task-1', 'pr-rework'] }],
          [{ queryKey: ['tasks', 'task-1', 'pr-actions'] }],
          [{ queryKey: ['tasks', 'task-1', 'human-decisions'] }],
        ]),
      )
    }
  })

  it('invalidates phase-three receipt, coordination, review and decision queries', () => {
    for (const name of [
      'task.work_unit_fallback',
      'task.micro_fix_bounds_exceeded',
      'task.coordinator_replan_proposed',
    ] as const) {
      const { queryClient, invalidateQueries } = createMocks()
      routeSsePayload(
        event(name, { scope_type: 'task', scope_id: 'task-1', payload: { task_id: 'task-1' } }),
        queryClient,
        { dispatch: vi.fn() },
      )
      expect(invalidateQueries.mock.calls).toEqual(
        expect.arrayContaining([
          [{ queryKey: ['tasks', 'task-1', 'fallback-receipts'] }],
          [{ queryKey: ['tasks', 'task-1', 'micro-fixes'] }],
          [{ queryKey: ['tasks', 'task-1', 'coordinator-replans'] }],
          [{ queryKey: ['tasks', 'task-1', 'human-decisions'] }],
          [{ queryKey: ['tasks', 'task-1', 'coordination'] }],
          [{ queryKey: ['tasks', 'task-1', 'review-lines'] }],
        ]),
      )
    }
  })
})
