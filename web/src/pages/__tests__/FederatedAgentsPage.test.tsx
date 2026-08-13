import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { FederatedAgentsPage } from '@/pages/FederatedAgentsPage'
import type { AgentChatEntry } from '@/features/agent-chat/types'
import type { FederatedAgent } from '@/features/federation/types'

const connect = vi.fn().mockResolvedValue({})

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}))
vi.mock('@/stores/auth', () => ({
  useAuthStore: (selector: (state: { user: null }) => unknown) => selector({ user: null }),
}))
vi.mock('@/features/federation/hooks', () => ({
  useFederatedAgentsQuery: () => ({
    data: { items: [agent], has_more: false },
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useAgentProfilesQuery: () => ({ data: [], isLoading: false, isError: false }),
  useAgentSessionsQuery: () => ({ data: [], isLoading: false, isError: false }),
  useSelectAgentProfileMutation: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useConnectEmbeddedAgentMutation: () => ({ mutateAsync: connect, isPending: false }),
  useMainAgentBindingQuery: () => ({
    data: undefined,
    isLoading: false,
    isError: false,
    refetch: vi.fn(),
  }),
  useSetMainAgentBindingMutation: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useContextManifestDiscoveryQuery: () => ({
    data: [],
    isLoading: false,
    isError: false,
    isFetching: false,
    refetch: vi.fn(),
  }),
  useContextManifestQuery: () => ({
    data: undefined,
    isLoading: false,
    isError: false,
    isFetching: false,
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
    identity_id: 'agent-1',
    identity_name: 'Forge Guide',
    binding_state: 'active',
    chat_status: 'ready',
    unread_count: 0n,
    pending_turn_count: 0n,
    last_message_at: null,
  },
  {
    chat_id: 'atlas-chat',
    kind: 'project',
    project_id: 'project-atlas',
    project_name: 'Atlas',
    identity_id: 'agent-1',
    identity_name: 'Forge Guide',
    binding_state: 'active',
    chat_status: 'ready',
    unread_count: 0n,
    pending_turn_count: 0n,
    last_message_at: null,
  },
]

const agent: FederatedAgent = {
  id: 'agent-1',
  name: 'Forge Guide',
  description: 'A bounded account assistant.',
  profile_id: 'profile-1',
  backend_kind: 'native',
  executor_type: 'embedded',
  provider: 'openai',
  model: 'gpt-test',
  reasoning_effort: null,
  permission_policy: 'scoped_proposals',
  prompt_template: null,
  capabilities: ['read_account'],
  config_json: {},
  credential_handle_id: 'credential-1',
  daemon_id: null,
  max_concurrent_tasks: 1,
  status: 'idle',
  active_task_count: 0,
  effective_status: 'ready',
  total_runs: 4,
  avg_duration_ms: 500,
  success_rate: 1,
  is_default: false,
  paused: false,
  owner_id: 'user-1',
  visibility: 'private',
  version: 1,
  created_at: '2026-08-12T11:00:00Z',
  updated_at: '2026-08-12T12:00:00Z',
}

describe('FederatedAgentsPage', () => {
  it('renders stable identity and connection state', () => {
    render(<FederatedAgentsPage />)
    expect(screen.getByText('Forge Guide')).toBeTruthy()
    expect(screen.getByText(/Forge-hosted runtime/)).toBeTruthy()
    expect(screen.getAllByText(/ready/i).length).toBeGreaterThan(0)
    expect(screen.getByText('Main and Project Agent bindings')).toBeTruthy()
    expect(screen.getByText('Global · Main')).toBeTruthy()
    expect(screen.getByText('Atlas')).toBeTruthy()
    expect(screen.getByRole('button', { name: /connect agent/i })).toBeTruthy()
  })

  it('submits the direct embedded-agent connection form', async () => {
    render(<FederatedAgentsPage />)
    fireEvent.click(screen.getByRole('button', { name: /connect agent/i }))
    fireEvent.change(screen.getByLabelText('Identity name'), { target: { value: 'Main guide' } })
    fireEvent.change(screen.getByLabelText('Credential'), { target: { value: 'secret-value' } })
    fireEvent.click(
      screen.getByRole('dialog').querySelector('button[type="submit"]') as HTMLButtonElement,
    )
    await vi.waitFor(() =>
      expect(connect).toHaveBeenCalledWith(
        expect.objectContaining({ name: 'Main guide', credential: 'secret-value' }),
      ),
    )
  })
})
