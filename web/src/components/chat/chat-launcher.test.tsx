import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ChatLauncher } from './chat-launcher'
import type { AgentChat, AgentChatEntry } from '@/features/agent-chat/types'
import { useChatSelection } from '@/stores/chat'

const mocks = vi.hoisted(() => ({
  useAgentChatsQuery: vi.fn(),
  useAgentChatQuery: vi.fn(),
  useCancelAgentChatTurnMutation: vi.fn(),
  useSendAgentChatMessageMutation: vi.fn(),
}))

vi.mock('@/features/agent-chat/hooks', () => mocks)
vi.mock('@/features/product-genesis/ProductGenesisControls', () => ({
  ProductGenesisControls: () => null,
}))
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: React.ReactNode }) => <a>{children}</a>,
}))
vi.mock('./agent-chat-timeline', () => ({
  AgentChatTimeline: ({
    chat,
    projectId,
    handoffProjectIds,
  }: {
    chat: AgentChat
    projectId?: string
    handoffProjectIds?: string[]
  }) => (
    <div
      data-testid="launcher-timeline"
      data-chat-id={chat.id}
      data-project-id={projectId}
      data-handoff-project-ids={handoffProjectIds?.join(',')}
    >
      {chat.title}
    </div>
  ),
}))

const mainEntry: AgentChatEntry = {
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
}

const projectEntry: AgentChatEntry = {
  ...mainEntry,
  chat_id: 'project-chat',
  kind: 'project',
  project_id: 'project-private',
  project_name: 'Private Project',
  identity_id: 'project-agent',
  identity_name: 'Project Agent',
}

const mainChat: AgentChat = {
  id: 'main-chat',
  kind: 'main',
  account_id: 'account-1',
  project_id: null,
  title: 'Main Agent Chat',
  status: 'ready',
  message_count: 2n,
  pending_turn_count: 0n,
  last_message_at: '2026-08-13T12:00:00Z',
  version: 1n,
  created_at: '2026-08-13T11:00:00Z',
  updated_at: '2026-08-13T12:00:00Z',
}

describe('ChatLauncher', () => {
  afterEach(() => {
    useChatSelection.getState().setGlobalChat(undefined)
    vi.clearAllMocks()
  })

  it('opens the canonical Main timeline without importing Project-private context', () => {
    mocks.useAgentChatsQuery.mockReturnValue({
      data: { items: [mainEntry, projectEntry] },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })
    mocks.useAgentChatQuery.mockImplementation((chatId: string | undefined) => ({
      data: chatId === 'main-chat' ? mainChat : undefined,
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    }))
    mocks.useSendAgentChatMessageMutation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    })
    mocks.useCancelAgentChatTurnMutation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    })

    // A stale persisted selection must not override the server's singular Main
    // entry, and the launcher must not receive a Project scope prop.
    useChatSelection.getState().setGlobalChat({ ...mainChat, id: 'stale-project-chat' })
    render(<ChatLauncher />)

    fireEvent.click(screen.getByRole('button', { name: 'Open global chat' }))

    const timeline = screen.getByTestId('launcher-timeline')
    expect(timeline.getAttribute('data-chat-id')).toBe('main-chat')
    expect(timeline.getAttribute('data-project-id')).toBeNull()
    expect(timeline.textContent).not.toContain('Private Project')
    expect(timeline.getAttribute('data-handoff-project-ids')).toBe('project-private')
  })

  it('does not open a stale local Project chat when the server has no Main entry', () => {
    mocks.useAgentChatsQuery.mockReturnValue({
      data: { items: [projectEntry] },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })
    mocks.useAgentChatQuery.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })
    mocks.useSendAgentChatMessageMutation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    })
    mocks.useCancelAgentChatTurnMutation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    })

    useChatSelection.getState().setGlobalChat({ ...mainChat, id: 'stale-project-chat' })
    render(<ChatLauncher />)
    fireEvent.click(screen.getByRole('button', { name: 'Open global chat' }))

    expect(screen.queryByTestId('launcher-timeline')).toBeNull()
    expect(screen.getByText(/setup required/i)).toBeTruthy()
  })

  it('moves focus into the panel and returns it to the launcher on Escape', async () => {
    mocks.useAgentChatsQuery.mockReturnValue({
      data: { items: [mainEntry] },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })
    mocks.useAgentChatQuery.mockReturnValue({
      data: mainChat,
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })
    mocks.useSendAgentChatMessageMutation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    })
    mocks.useCancelAgentChatTurnMutation.mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    })

    render(<ChatLauncher />)
    const launcher = screen.getByRole('button', { name: 'Open global chat' })
    fireEvent.click(launcher)

    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('heading', { name: 'Global · Main' }),
      ),
    )
    fireEvent.keyDown(document, { key: 'Escape' })
    await waitFor(() => expect(document.activeElement).toBe(launcher))
    expect(screen.queryByRole('dialog')).toBeNull()
  })
})
