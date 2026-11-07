import { Robot, UserCircle } from '@phosphor-icons/react'
import { CopyReferenceButton } from '@/components/ui/copy-reference-button'
import { cn } from '@/lib/cn'
import type {
  ActorRef,
  AgentResponse,
  DaemonResponse,
  ProjectMemberResponse,
} from '@/types/generated'

export function resolveActorIdentity(
  actor: ActorRef,
  agents: AgentResponse[],
  members: ProjectMemberResponse[],
  daemons: DaemonResponse[],
) {
  if (actor.kind === 'human') {
    const member = members.find((item) => item.user_id === actor.id)
    return {
      name: member?.display_name?.trim() || member?.email || 'Project member',
      kind: 'Human',
      context: member?.email ?? 'Project member',
    }
  }

  const agent = agents.find((item) => item.id === actor.id)
  const host = agent?.daemon_id ? daemons.find((item) => item.id === agent.daemon_id) : undefined
  return {
    name: agent?.name?.trim() || 'Agent',
    kind: 'Agent',
    context: [
      agent?.executor_type.replaceAll('_', ' ') || 'Harness unavailable',
      host?.hostname ?? (agent?.daemon_id ? 'Assigned host unavailable' : 'Host not pinned'),
    ].join(' · '),
  }
}

export function ActorIdentity({
  actor,
  agents,
  members,
  daemons,
  className,
}: {
  actor: ActorRef
  agents: AgentResponse[]
  members: ProjectMemberResponse[]
  daemons: DaemonResponse[]
  className?: string
}) {
  const identity = resolveActorIdentity(actor, agents, members, daemons)
  const ActorIcon = actor.kind === 'agent' ? Robot : UserCircle

  return (
    <div className={cn('min-w-0', className)}>
      <div className="flex min-w-0 items-center gap-2">
        <ActorIcon size={17} className="shrink-0 text-muted-foreground" aria-hidden="true" />
        <div className="min-w-0">
          <p className="truncate text-sm font-medium text-foreground" title={identity.name}>
            {identity.name}
          </p>
          <p className="truncate text-xs text-muted-foreground" title={identity.context}>
            {identity.kind} · {identity.context}
          </p>
        </div>
      </div>
      <details className="ml-6 mt-1 text-xs text-muted-foreground">
        <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          Technical reference
        </summary>
        <div className="mt-1 flex min-w-0 items-center gap-1">
          <code className="min-w-0 break-all">{actor.id}</code>
          <CopyReferenceButton value={actor.id} label={`${identity.kind} ID`} />
        </div>
      </details>
    </div>
  )
}
