import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { PolicyBadge } from '@/components/policy-badge'
import type { EffectiveExecutionPolicy } from '@/types/generated'

function policy(overrides: Partial<EffectiveExecutionPolicy> = {}): EffectiveExecutionPolicy {
  return {
    executor_kind: 'codex_cli',
    permission_policy: 'on_request',
    isolation_posture: 'workspace_write',
    is_high_risk: false,
    effective_cwd: '/repo',
    workspace_root: '/repo',
    environment_posture: 'network_restricted',
    scoped_tools: [],
    mcp_servers: [],
    ...overrides,
  }
}

describe('PolicyBadge', () => {
  it('renders executor kind badge', () => {
    render(<PolicyBadge policy={policy()} />)

    expect(screen.getByText('Codex Cli')).toBeTruthy()
  })

  it('renders high-risk warning badge when is_high_risk is true', () => {
    render(<PolicyBadge policy={policy({ is_high_risk: true })} />)

    expect(screen.getByText('High Risk')).toBeTruthy()
  })

  it('does not render high-risk badge when is_high_risk is false', () => {
    render(<PolicyBadge policy={policy({ is_high_risk: false })} />)

    expect(screen.queryByText('High Risk')).toBeNull()
  })
})
