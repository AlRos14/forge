import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { MissionControlPage } from '@/pages/MissionControlPage'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import type { MissionControlResponse } from '@/features/federation/types'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}))
vi.mock('@/features/federation/hooks', () => ({
  useMissionControlQuery: () => ({
    data,
    isLoading: false,
    isError: false,
    isFetching: false,
    dataUpdatedAt: Date.now(),
    refetch: vi.fn(),
  }),
}))
vi.mock('@/features/agent-chat/hooks', () => ({
  useAgentChatsQuery: () => ({
    data: { items: chatEntries },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
}))

const chatEntries: AgentChatEntry[] = [
  {
    chat_id: 'main-chat',
    kind: 'main',
    project_id: null,
    project_name: null,
    identity_id: 'main-agent',
    identity_name: 'Main Agent',
    binding_state: 'active',
    chat_status: 'ready',
    unread_count: 0n,
    pending_turn_count: 0n,
    last_message_at: null,
  },
]

const data: MissionControlResponse = {
  needs_attention: [
    {
      id: 'attention-1',
      category: 'review_risk',
      scope_type: 'task',
      scope_id: 'task-1',
      identity_id: null,
      source_event_id: 'event-1',
      priority: 80,
      lifecycle: 'open',
      summary: 'A human decision is required.',
      details: {},
      dedupe_key: 'review:task-1',
      occurred_at: '2026-08-12T12:00:00Z',
      updated_at: '2026-08-12T12:00:00Z',
      version: 1,
      acknowledged_at: null,
      snoozed_until: null,
      resolved_at: null,
      recommended_action: 'Open review',
    },
  ],
  review_ready: [
    {
      task_id: 'task-1',
      title: 'Ship the Project worker',
      project_id: 'project-1',
      status: 'awaiting_human',
      priority: 80,
      primary_action: 'Open review',
      updated_at: '2026-08-12T12:00:00Z',
    },
  ],
  active_work: [],
  agent_health: [
    {
      identity_id: 'main-agent',
      name: 'Main Agent identity',
      backend_kind: 'native',
      provider: 'forge',
      model: 'main-model',
      identity_status: 'ready',
      paused: false,
      connection_status: 'healthy',
      last_activity_at: '2026-08-12T12:00:00Z',
      active_session_count: 0,
      project_count: 0,
    },
    {
      identity_id: 'unbound-profile',
      name: 'Unbound profile',
      backend_kind: 'native',
      provider: 'forge',
      model: 'unused-model',
      identity_status: 'ready',
      paused: false,
      connection_status: 'healthy',
      last_activity_at: null,
      active_session_count: 0,
      project_count: 0,
    },
    {
      identity_id: 'task-worker',
      name: 'Task Worker',
      backend_kind: 'native',
      provider: 'forge',
      model: 'worker-model',
      identity_status: 'ready',
      paused: false,
      connection_status: 'healthy',
      last_activity_at: '2026-08-12T12:00:00Z',
      active_session_count: 1,
      project_count: 1,
    },
  ],
  recent_outcomes: [],
  capacity: { active_executions: 1, queued_tasks: 0, active_sessions: 4, healthy: true },
  consumer_health: null,
  computed_at: '2026-08-12T12:00:00Z',
}

describe('MissionControlPage', () => {
  it('prioritizes attention and review-ready work', () => {
    render(<MissionControlPage />)
    expect(screen.getByText('What needs your attention?')).toBeTruthy()
    expect(screen.getByText('A human decision is required.')).toBeTruthy()
    expect(screen.getByText('Ship the Project worker')).toBeTruthy()
    expect(screen.getByText('Main and Project Agent bindings')).toBeTruthy()
    expect(screen.getByText('Global · Main')).toBeTruthy()
    expect(screen.getByText('Main Agent identity')).toBeTruthy()
    expect(screen.getByText('Task Worker')).toBeTruthy()
    expect(screen.queryByText('Unbound profile')).toBeNull()
    const capacity =
      screen.getByText('Runtime capacity').parentElement?.parentElement?.parentElement?.textContent
    expect(capacity).toContain('1')
    expect(capacity).toContain('4')
  })
})
