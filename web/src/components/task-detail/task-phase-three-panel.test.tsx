import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  CoordinatorReplanResponse,
  MicroFixReceiptResponse,
  Task,
  WorkUnitFallbackReceiptResponse,
} from '@/types/generated'
import { TaskPhaseThreePanel } from './task-phase-three-panel'

const queries = vi.hoisted(() => ({
  fallback: {} as Record<string, unknown>,
  microFixes: {} as Record<string, unknown>,
  replans: {} as Record<string, unknown>,
}))

vi.mock('@/api/hooks', () => ({
  useTaskFallbackReceiptsQuery: () => queries.fallback,
  useTaskMicroFixesQuery: () => queries.microFixes,
  useTaskCoordinatorReplansQuery: () => queries.replans,
}))

const task = {
  id: 'task-1',
  project_id: 'project-1',
  repo_id: null,
  repositories: [],
  repository_set_revision: 0,
  repository_set_digest: null,
  repository_merge_batches: [],
} as unknown as Task

const fallbackReceipt = {
  id: 'fallback-1',
  task_id: 'task-1',
  work_unit_id: 'work-unit-1',
  work_unit_version_before: 2,
  work_unit_version_after: 3,
  origin_execution_id: 'execution-1',
  failure_class: 'executor_unavailable',
  route_attempts_digest: 'digest-route',
  task_role_id: 'role-1',
  from_actor_kind: 'agent',
  from_actor_id: 'agent-original',
  to_actor_kind: 'agent',
  to_actor_id: 'agent-fallback',
  chain_index: 0,
  cost_class: 'economy',
  authorization_version: 4,
  envelope_digest: 'digest-envelope',
  new_execution_id: null,
  created_at: '2026-11-17T10:00:00Z',
} as WorkUnitFallbackReceiptResponse

const microFix = {
  id: 'micro-fix-1',
  task_id: 'task-1',
  request_id: 'request-1',
  source_kind: 'review_findings',
  finding_refs: ['finding-1'],
  declared_paths: ['src/lib.rs'],
  fix_work_unit_id: 'work-unit-fix',
  reviewed_commit_sha: 'abc123',
  result_commit_sha: 'def456',
  changed_files: 4,
  changed_lines: 65,
  bounds_outcome: 'bounds_exceeded',
  bounds_violation: 'too_many_lines',
  reviewer_cost_class: 'standard',
  reviewer_actor_kind: 'agent',
  reviewer_actor_id: 'agent-reviewer',
  envelope_digest: 'digest-envelope',
  ordinal: 1,
  created_at: '2026-11-17T10:00:00Z',
  updated_at: '2026-11-17T10:01:00Z',
} as MicroFixReceiptResponse

const replan = {
  id: 'replan-1',
  task_id: 'task-1',
  ordinal: 2,
  authorization_version: 4,
  source_request_id: 'source-request-1',
  producer_execution_id: 'coordinator-execution-1',
  plan_request_id: 'plan-request-1',
  reason: 'The original objective needs a wider scope.',
  evidence_links: [],
  classification: 'scope_or_authority_change',
  classification_facts: { objective_digest_matches: true, roles_within_team: false },
  human_decision_id: 'decision-12345678',
  state: 'awaiting_human',
  version: 1,
  created_at: '2026-11-17T10:00:00Z',
  updated_at: '2026-11-17T10:00:00Z',
} as CoordinatorReplanResponse

function loaded(items: unknown[]) {
  return {
    data: { pages: [{ items, next_cursor: null }], pageParams: [] },
    isLoading: false,
    isError: false,
    isFetchingNextPage: false,
    hasNextPage: false,
    refetch: vi.fn(),
    fetchNextPage: vi.fn(),
  }
}

describe('TaskPhaseThreePanel', () => {
  beforeEach(() => {
    queries.fallback = loaded([fallbackReceipt])
    queries.microFixes = loaded([microFix])
    queries.replans = loaded([replan])
  })

  it('shows the fallback substitution, cost class and micro-fix bounds result', () => {
    render(<TaskPhaseThreePanel task={task} />)

    expect(screen.getByText('Agent agent-original replaced by Agent agent-fallback')).toBeTruthy()
    expect(screen.getByText('economy')).toBeTruthy()
    expect(screen.getByText('executor unavailable')).toBeTruthy()
    expect(screen.getByText('bounds exceeded')).toBeTruthy()
    expect(screen.getByText('too many lines')).toBeTruthy()
    expect(screen.getByText(/4 files · 65 changed lines/)).toBeTruthy()
  })

  it('shows replan ordinal, classification facts and its Human decision link', () => {
    render(<TaskPhaseThreePanel task={task} />)

    expect(screen.getByText('Replan 2')).toBeTruthy()
    expect(screen.getByText('scope or authority change')).toBeTruthy()
    expect(screen.getByText(/objective_digest_matches/)).toBeTruthy()
    const link = screen.getByRole('link', { name: /Human decision decision/ })
    expect(link.getAttribute('href')).toBe('#human-decision-decision-12345678')
  })
})
