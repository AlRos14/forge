import { describe, expect, it } from 'vitest'
import type { ActorRef, ProjectTaskRoleDefault } from '@/types/generated'
import { recommendedWorkflowRoles } from './recommended-workflow'

const shared: ActorRef = { kind: 'agent', id: 'deepseek' }
const coordinator: ActorRef = { kind: 'agent', id: 'sol' }
const roles: ProjectTaskRoleDefault[] = [
  {
    role: 'build',
    coordination_mode: 'independent',
    members: [shared],
    configuration: { schema_version: 1, display_label: 'Build', capabilities: [], rework: null },
  },
  {
    role: 'quality',
    coordination_mode: 'independent',
    members: [shared],
    configuration: {
      schema_version: 1,
      display_label: 'Quality',
      capabilities: ['formal_review'],
      rework: null,
    },
  },
  {
    role: 'lead',
    coordination_mode: 'independent',
    members: [coordinator],
    configuration: {
      schema_version: 1,
      display_label: 'Lead',
      capabilities: ['orchestration'],
      rework: null,
    },
  },
]

describe('recommended delivery workflow', () => {
  it('uses capabilities and explicit destinations with a shared Agent profile', () => {
    const before = structuredClone(roles)
    const result = recommendedWorkflowRoles(
      roles,
      'build',
      { role: 'lead', actor: coordinator },
      {},
    )
    expect(result[1].configuration.rework).toEqual({
      mode: 'automatic',
      allowed_destination_roles: ['build'],
      next_review_role: 'quality',
      correction_actor: { kind: 'original_contributor' },
      review_actor: shared,
    })
    expect(result[1].configuration.capabilities).toEqual(['formal_review', 'request_rework'])
    expect(result[2].configuration.coordinator_actor).toEqual(coordinator)
    expect(result[0].configuration.coordinator_actor).toBeNull()
    expect(roles).toEqual(before)
  })

  it('retains an explicit reviewer rather than selecting another identity automatically', () => {
    const reviewer: ActorRef = { kind: 'agent', id: 'chosen-reviewer' }
    const result = recommendedWorkflowRoles(roles, 'build', null, { quality: reviewer })
    expect(result[1].configuration.rework?.review_actor).toEqual(reviewer)
    expect(result.every((role) => role.configuration.coordinator_actor === null)).toBe(true)
  })
})
