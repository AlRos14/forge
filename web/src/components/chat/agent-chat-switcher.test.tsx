import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { AgentChatSwitcher } from './agent-chat-switcher'

describe('AgentChatSwitcher', () => {
  it('renders one Main entry and one entry per Project without identity/new-chat groups', () => {
    const onSelectGlobal = vi.fn()
    const onSelectProject = vi.fn()

    render(
      <AgentChatSwitcher
        globalEntry={{
          id: 'main-chat',
          label: 'Global · Main',
          description: 'Account-owned Main Agent timeline',
          agentName: 'Main',
          agentStatus: 'ready',
          active: true,
        }}
        projectEntries={[
          {
            id: 'project-chat',
            label: 'Atlas',
            description: 'Project-owned Agent timeline',
            agentName: 'Atlas Agent',
            agentStatus: 'ready',
            pendingTurnCount: 1,
          },
        ]}
        onSelectGlobal={onSelectGlobal}
        onSelectProject={onSelectProject}
      />,
    )

    expect(screen.getByRole('button', { name: /Global · Main/ }).getAttribute('aria-current')).toBe(
      'page',
    )
    expect(
      screen.getByRole('button', { name: /Atlas: Project-owned.*1 turn pending/ }),
    ).toBeTruthy()
    expect(screen.getByText('1 turn pending')).toBeTruthy()
    expect(screen.queryByText(/Start a new chat|No eligible agents/)).toBeNull()
    expect(screen.queryByRole('button', { name: /Atlas Agent/ })).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: /Atlas: Project-owned/ }))
    expect(onSelectProject).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'project-chat', label: 'Atlas' }),
    )
    fireEvent.click(screen.getByRole('button', { name: /Global · Main/ }))
    expect(onSelectGlobal).toHaveBeenCalledTimes(1)
  })

  it('keeps setup-required state visible without inventing an identity', () => {
    render(
      <AgentChatSwitcher
        globalEntry={{
          id: 'main-chat',
          label: 'Global · Main',
          description: 'Account-owned Main Agent timeline',
          setupRequired: true,
        }}
        projectEntries={[]}
        onSelectGlobal={vi.fn()}
        onSelectProject={vi.fn()}
      />,
    )

    expect(screen.getByRole('button', { name: /Global · Main:.*Setup required/ })).toBeTruthy()
    expect(screen.getByText('Setup required')).toBeTruthy()
    expect(screen.getByText('No Project Agent chats are available yet.')).toBeTruthy()
    expect(screen.getByRole('navigation', { name: 'Agent chat switcher' })).toBeTruthy()
  })

  it('distinguishes loading and unavailable Project chat lists', () => {
    const { rerender } = render(
      <AgentChatSwitcher
        globalEntry={{
          id: 'main-chat',
          label: 'Global · Main',
          description: 'Account-owned Main Agent timeline',
          state: 'loading',
        }}
        projectEntries={[]}
        projectListState="loading"
        onSelectGlobal={vi.fn()}
        onSelectProject={vi.fn()}
      />,
    )

    expect(screen.getByRole('button', { name: /Global · Main:.*Loading/ }).hasAttribute('disabled')).toBe(true)
    expect(screen.getByText('Loading Project Agent chats…')).toBeTruthy()

    rerender(
      <AgentChatSwitcher
        globalEntry={{
          id: 'main-chat',
          label: 'Global · Main',
          description: 'Account-owned Main Agent timeline',
          state: 'unavailable',
        }}
        projectEntries={[]}
        projectListState="error"
        onSelectGlobal={vi.fn()}
        onSelectProject={vi.fn()}
      />,
    )

    expect(screen.getByRole('button', { name: /Global · Main:.*Unavailable/ })).toBeTruthy()
    expect(screen.getByRole('alert').textContent).toContain('Project Agent chats unavailable')
  })
})
