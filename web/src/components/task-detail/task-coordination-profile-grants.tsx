import { Badge } from '@/components/ui/badge'
import type { AgentResponse, TaskCoordinationOverviewResponse } from '@/types/generated'
import { coordinationRoleLabel } from './task-coordination-status'

type Grant = { task_role_id: string; agent_id: string; cost_class: string }

function isJsonObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function readGrants(envelope: unknown): { fallback: Grant[]; reviewer: Grant | null } {
  if (!isJsonObject(envelope)) return { fallback: [], reviewer: null }
  const fallback = Array.isArray(envelope.fallback_agents)
    ? envelope.fallback_agents.filter(
        (grant): grant is Grant =>
          isJsonObject(grant) &&
          typeof grant.task_role_id === 'string' &&
          typeof grant.agent_id === 'string' &&
          typeof grant.cost_class === 'string',
      )
    : []
  const candidate = envelope.micro_fix_reviewer
  const reviewer =
    isJsonObject(candidate) &&
    typeof candidate.task_role_id === 'string' &&
    typeof candidate.agent_id === 'string' &&
    typeof candidate.cost_class === 'string'
      ? {
          task_role_id: candidate.task_role_id,
          agent_id: candidate.agent_id,
          cost_class: candidate.cost_class,
        }
      : null
  return { fallback, reviewer }
}

export function TaskCoordinationProfileGrants({
  envelope,
  team,
  agents,
}: {
  envelope: unknown
  team: TaskCoordinationOverviewResponse['team']
  agents: AgentResponse[]
}) {
  const grants = readGrants(envelope)
  if (grants.fallback.length === 0 && !grants.reviewer) return null

  const agentName = (agentId: string) => agents.find((agent) => agent.id === agentId)?.name ?? `Agent ${agentId}`
  const roleName = (roleId: string) => {
    const role = team.find((item) => item.id === roleId)
    return role ? coordinationRoleLabel(role) : roleId
  }

  return (
    <section className="space-y-2 rounded-md border p-3" aria-labelledby="coordination-profile-grants-title">
      <div>
        <h3 id="coordination-profile-grants-title" className="text-sm font-semibold">
          Preauthorized delivery profiles
        </h3>
        <p className="mt-1 text-xs text-muted-foreground">
          These profiles are fixed by the current coordination envelope.
        </p>
      </div>
      <ul className="space-y-2 text-sm">
        {grants.fallback.map((grant, index) => (
          <li key={`${grant.task_role_id}:${grant.agent_id}`} className="flex flex-wrap items-center gap-2">
            <span>
              Fallback {index + 1} · {roleName(grant.task_role_id)} · {agentName(grant.agent_id)}
            </span>
            <Badge variant="outline">{grant.cost_class}</Badge>
          </li>
        ))}
        {grants.reviewer ? (
          <li className="flex flex-wrap items-center gap-2">
            <span>
              Micro-fix reviewer · {roleName(grants.reviewer.task_role_id)} · {agentName(grants.reviewer.agent_id)}
            </span>
            <Badge variant="outline">{grants.reviewer.cost_class}</Badge>
          </li>
        ) : null}
      </ul>
    </section>
  )
}
