import type { ActorRef, ProjectTaskRoleDefault } from '@/types/generated'

export function recommendedWorkflowRoles(
  roles: ProjectTaskRoleDefault[],
  implementationRole: string,
  coordinator: { role: string; actor: ActorRef } | null,
  reviewers: Record<string, ActorRef>,
): ProjectTaskRoleDefault[] {
  return roles.map((role) => {
    const primary = coordinator?.role === role.role ? coordinator.actor : null
    if (!role.configuration.capabilities.includes('formal_review')) {
      return { ...role, configuration: { ...role.configuration, coordinator_actor: primary } }
    }
    const reviewer =
      reviewers[role.role] ??
      role.configuration.rework?.review_actor ??
      role.members.find((actor) => actor.kind === 'agent') ??
      null
    return {
      ...role,
      configuration: {
        ...role.configuration,
        coordinator_actor: primary,
        capabilities: [...new Set([...role.configuration.capabilities, 'request_rework' as const])],
        rework: {
          mode: 'automatic',
          allowed_destination_roles: [implementationRole],
          next_review_role: role.role,
          correction_actor: { kind: 'original_contributor' },
          review_actor: reviewer,
        },
      },
    }
  })
}
