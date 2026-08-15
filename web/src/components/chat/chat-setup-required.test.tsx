import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { ChatSetupRequired } from './chat-setup-required'

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, to, params }: { children: React.ReactNode; to: string; params?: unknown }) => (
    <a href={to} data-params={JSON.stringify(params)}>
      {children}
    </a>
  ),
}))

describe('ChatSetupRequired', () => {
  it('keeps a Project chat visible but blocks turns until its sole binding is configured', () => {
    render(<ChatSetupRequired projectId="project-1" />)

    expect(screen.getByRole('status').textContent).toContain(
      'Project Agent binding is not ready yet',
    )
    expect(
      screen.getByRole('link', { name: /Open Project Agent settings/ }).getAttribute('href'),
    ).toBe('/agents')
  })

  it('points Main setup to the binding screen without inventing an identity', () => {
    render(<ChatSetupRequired />)

    expect(screen.getByRole('status').textContent).toContain('no Main Agent binding is configured')
    expect(
      screen.getByRole('link', { name: /Open Main Agent settings/ }).getAttribute('href'),
    ).toBe('/agents')
  })
})
