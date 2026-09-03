import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { ChatAssistantMessage } from './chat-assistant-message'

function renderMessage(text: string) {
  render(
    <ChatAssistantMessage
      entry={{
        kind: 'assistant',
        sequence: 1,
        timestamp: '2026-09-02T00:00:00Z',
        text,
      }}
    />,
  )
}

describe('ChatAssistantMessage', () => {
  it('opens captured plan files through the task plan UI', () => {
    renderMessage(
      'Implementation plan written to [plan.md](/tmp/forge/worktrees/6ca7d024-77a5-4e00-be97-f0b889ed6a67/plan.md).',
    )

    expect(screen.getByRole('link', { name: 'plan.md' }).getAttribute('href')).toBe(
      '/tasks/6ca7d024-77a5-4e00-be97-f0b889ed6a67/overview#task-plan',
    )
  })

  it('preserves ordinary web links', () => {
    renderMessage('[Documentation](https://example.com/docs)')

    expect(screen.getByRole('link', { name: 'Documentation' }).getAttribute('href')).toBe(
      'https://example.com/docs',
    )
  })
})
