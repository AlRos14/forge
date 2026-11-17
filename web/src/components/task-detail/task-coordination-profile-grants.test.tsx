import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { AgentResponse, TaskCoordinationOverviewResponse } from '@/types/generated'
import { TaskCoordinationProfileGrants } from './task-coordination-profile-grants'

const team = [
  { id: 'role-implementer', role: 'implementer', display_label: 'Implementer' },
] as unknown as TaskCoordinationOverviewResponse['team']
const agents = [
  { id: 'agent-economy', name: 'Economy Agent' },
  { id: 'agent-reviewer', name: 'Review Agent' },
] as unknown as AgentResponse[]

describe('TaskCoordinationProfileGrants', () => {
  it('shows ordered fallback and micro-fix reviewer cost classes from the envelope', () => {
    render(
      <TaskCoordinationProfileGrants
        envelope={{
          fallback_agents: [
            { task_role_id: 'role-implementer', agent_id: 'agent-economy', cost_class: 'economy' },
          ],
          micro_fix_reviewer: {
            task_role_id: 'role-implementer',
            agent_id: 'agent-reviewer',
            cost_class: 'standard',
          },
        }}
        team={team}
        agents={agents}
      />,
    )

    expect(screen.getByText(/Fallback 1 · Implementer · Economy Agent/)).toBeTruthy()
    expect(screen.getByText('economy')).toBeTruthy()
    expect(screen.getByText(/Micro-fix reviewer · Implementer · Review Agent/)).toBeTruthy()
    expect(screen.getByText('standard')).toBeTruthy()
  })
})
