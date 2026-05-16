import { useMemo, useState } from 'react'
import { CircleNotch, LinkBreak, MagnifyingGlass, Plus, Robot } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useAgentsQuery,
  useCreateProjectAgentLink,
  useDeleteProjectAgentLink,
  useMembersQuery,
  useProjectAgentLinksQuery,
  useProjectAgentsQuery,
} from '@/api/hooks'
import { ErrorBanner } from '@/components/error-banner'
import { Avatar } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { getApiErrorMessage } from '@/lib/api-error'
import { cn } from '@/lib/cn'
import { useAuthStore } from '@/stores/auth'
import type {
  Agent,
  ProjectAgentLinkResponse,
  ProjectMemberResponse,
  UserResponse,
} from '@/types/generated'

const STATUS_DOT_CLASSES: Record<string, string> = {
  idle: 'bg-emerald-500',
  busy: 'bg-sky-500',
  error: 'bg-red-500',
  offline: 'bg-zinc-400',
  paused: 'bg-amber-500',
}

function formatLabel(value: string | null | undefined): string {
  return value ? value.replace(/_/g, ' ') : 'Unknown'
}

function compactId(id: string): string {
  return id.length > 12 ? `${id.slice(0, 8)}...` : id
}

function memberLabel(member: ProjectMemberResponse): string {
  return member.display_name ?? member.email
}

function userLabel(
  userId: string | null | undefined,
  membersByUserId: Map<string, ProjectMemberResponse>,
  currentUser: UserResponse | null,
): string {
  if (!userId) return 'System'
  const member = membersByUserId.get(userId)
  if (member) return memberLabel(member)
  if (currentUser?.id === userId) return currentUser.display_name ?? currentUser.email
  return compactId(userId)
}

function agentStatus(agent?: Agent): string {
  return agent?.effective_status ?? agent?.status ?? 'unknown'
}

function statusDotClass(status: string): string {
  return STATUS_DOT_CLASSES[status.toLowerCase()] ?? STATUS_DOT_CLASSES.offline
}

function agentMatches(agent: Agent, query: string): boolean {
  const normalized = query.trim().toLowerCase()
  if (!normalized) return true
  return (
    agent.name.toLowerCase().includes(normalized) ||
    agent.executor_type.toLowerCase().includes(normalized) ||
    agent.status.toLowerCase().includes(normalized)
  )
}

function LinkAgentDialog({
  open,
  onOpenChange,
  projectId,
  linkedAgentIds,
  membersByUserId,
  currentUser,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  projectId: string
  linkedAgentIds: Set<string>
  membersByUserId: Map<string, ProjectMemberResponse>
  currentUser: UserResponse | null
}) {
  const [query, setQuery] = useState('')
  const agentsQuery = useAgentsQuery()
  const createLink = useCreateProjectAgentLink(projectId)

  const availableAgents = useMemo(
    () => (agentsQuery.data?.items ?? []).filter((agent) => !linkedAgentIds.has(agent.id)),
    [agentsQuery.data?.items, linkedAgentIds],
  )
  const filteredAgents = useMemo(
    () => availableAgents.filter((agent) => agentMatches(agent, query)),
    [availableAgents, query],
  )

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) setQuery('')
    onOpenChange(nextOpen)
  }

  const linkAgent = (agent: Agent) => {
    createLink.mutate(agent.id, {
      onError: (error) => toast.error(getApiErrorMessage(error, 'Failed to link agent')),
      onSuccess: () => {
        toast.success('Agent linked')
        handleOpenChange(false)
      },
    })
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>Link agent</DialogTitle>
          <DialogDescription>
            Select an agent visible to you to make it usable in this project.
          </DialogDescription>
        </DialogHeader>

        <div className="mt-4 space-y-3">
          <div className="relative">
            <MagnifyingGlass
              size={14}
              className="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              value={query}
              className="pl-8"
              placeholder="Search agents"
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>

          {agentsQuery.isError ? (
            <ErrorBanner
              error={agentsQuery.error}
              fallback="Agents failed to load"
              onRetry={() => void agentsQuery.refetch()}
            />
          ) : agentsQuery.isLoading ? (
            <div className="space-y-2">
              <Skeleton className="h-14 w-full" />
              <Skeleton className="h-14 w-full" />
              <Skeleton className="h-14 w-full" />
            </div>
          ) : filteredAgents.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border py-8 text-center">
              <Robot size={24} className="mx-auto mb-2 text-muted-foreground/60" />
              <p className="text-sm font-medium text-foreground">No agents available</p>
              <p className="mt-1 text-sm text-muted-foreground">
                All visible agents are already linked or do not match the search.
              </p>
            </div>
          ) : (
            <div className="max-h-[360px] space-y-2 overflow-y-auto pr-1">
              {filteredAgents.map((agent) => {
                const status = agentStatus(agent)
                const pending = createLink.isPending && createLink.variables === agent.id
                return (
                  <button
                    key={agent.id}
                    type="button"
                    disabled={createLink.isPending}
                    className="flex w-full cursor-pointer items-center gap-3 rounded-lg border border-border-subtle bg-card px-3 py-3 text-left transition-colors hover:bg-accent/50 disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={() => linkAgent(agent)}
                  >
                    <Avatar name={agent.name} seed={agent.id} size="md" />
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm font-medium text-foreground">{agent.name}</p>
                      <div className="mt-1 flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
                        <Badge variant="outline" className="capitalize">
                          {formatLabel(agent.executor_type)}
                        </Badge>
                        <span className="inline-flex items-center gap-1.5 capitalize">
                          <span
                            className={cn('h-1.5 w-1.5 rounded-full', statusDotClass(status))}
                          />
                          {formatLabel(status)}
                        </span>
                        <span>{formatLabel(agent.visibility)}</span>
                        <span>Owner {userLabel(agent.owner_id, membersByUserId, currentUser)}</span>
                      </div>
                    </div>
                    <span className="shrink-0 text-xs font-medium text-primary">
                      {pending ? 'Linking...' : 'Link'}
                    </span>
                  </button>
                )
              })}
            </div>
          )}
        </div>

        <DialogFooter className="mt-4">
          <Button variant="outline" onClick={() => handleOpenChange(false)}>
            Cancel
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function LinkedAgentRow({
  link,
  agent,
  canManage,
  membersByUserId,
  currentUser,
  unlinkPending,
  pendingAgentId,
  onUnlink,
}: {
  link: ProjectAgentLinkResponse
  agent?: Agent
  canManage: boolean
  membersByUserId: Map<string, ProjectMemberResponse>
  currentUser: UserResponse | null
  unlinkPending: boolean
  pendingAgentId?: string
  onUnlink: (agentId: string) => void
}) {
  const name = agent?.name ?? link.agent_id
  const status = agentStatus(agent)
  const linkedBy = userLabel(link.linked_by_user_id, membersByUserId, currentUser)

  return (
    <div className="flex items-center gap-3 rounded-lg border border-border-subtle bg-card px-4 py-3">
      <Avatar name={name} seed={link.agent_id} size="md" />
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <p className="truncate text-sm font-semibold text-foreground">{name}</p>
          <Badge variant="outline" className="capitalize">
            {formatLabel(agent?.executor_type)}
          </Badge>
          <Badge variant="secondary" className="capitalize">
            <span className={cn('mr-1.5 h-1.5 w-1.5 rounded-full', statusDotClass(status))} />
            {formatLabel(status)}
          </Badge>
        </div>
        <div className="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
          <span className="truncate">
            Owner {agent ? userLabel(agent.owner_id, membersByUserId, currentUser) : 'Unknown'}
          </span>
          <span className="capitalize">Visibility {formatLabel(agent?.visibility)}</span>
          <span className="truncate">Linked by {linkedBy}</span>
        </div>
      </div>
      {canManage ? (
        <Button
          size="sm"
          variant="outline"
          disabled={unlinkPending}
          className="shrink-0 text-destructive hover:bg-destructive/10 hover:text-destructive"
          onClick={() => onUnlink(link.agent_id)}
        >
          {unlinkPending && pendingAgentId === link.agent_id ? (
            <CircleNotch size={13} className="mr-1.5 animate-spin" />
          ) : (
            <LinkBreak size={13} className="mr-1.5" />
          )}
          Unlink
        </Button>
      ) : null}
    </div>
  )
}

export function LinkedAgentsTab({ projectId }: { projectId: string }) {
  const currentUser = useAuthStore((state) => state.user)
  const membersQuery = useMembersQuery(projectId)
  const linksQuery = useProjectAgentLinksQuery(projectId)
  const projectAgentsQuery = useProjectAgentsQuery(projectId)
  const deleteLink = useDeleteProjectAgentLink(projectId)
  const [linkDialogOpen, setLinkDialogOpen] = useState(false)

  const members = membersQuery.data ?? []
  const links = linksQuery.data ?? []
  const projectAgents = projectAgentsQuery.data ?? []
  const currentMember = members.find((member) => member.user_id === currentUser?.id)
  const canManage = currentMember?.role === 'owner' || currentMember?.role === 'admin'

  const linkedAgentIds = useMemo(() => new Set(links.map((link) => link.agent_id)), [links])
  const membersByUserId = useMemo(
    () => new Map(members.map((member) => [member.user_id, member])),
    [members],
  )
  const projectAgentsById = useMemo(
    () => new Map(projectAgents.map((agent) => [agent.id, agent])),
    [projectAgents],
  )

  const unlinkAgent = (agentId: string) => {
    deleteLink.mutate(agentId, {
      onError: (error) => toast.error(getApiErrorMessage(error, 'Failed to unlink agent')),
      onSuccess: () => toast.success('Agent unlinked'),
    })
  }

  const isLoading = linksQuery.isLoading || projectAgentsQuery.isLoading || membersQuery.isLoading

  return (
    <>
      <div className="mb-8 flex items-start justify-between gap-4">
        <div>
          <h2 className="text-page font-semibold tracking-tight">Linked Agents</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Explicit agents shared with this project.
          </p>
        </div>
        {canManage ? (
          <Button onClick={() => setLinkDialogOpen(true)}>
            <Plus size={14} className="mr-1.5" />
            Link Agent
          </Button>
        ) : null}
      </div>

      <div className="space-y-3">
        {linksQuery.isError ? (
          <ErrorBanner
            error={linksQuery.error}
            fallback="Linked agents failed to load"
            onRetry={() => void linksQuery.refetch()}
          />
        ) : null}
        {projectAgentsQuery.isError ? (
          <ErrorBanner
            error={projectAgentsQuery.error}
            fallback="Agent details failed to load"
            onRetry={() => void projectAgentsQuery.refetch()}
          />
        ) : null}
        {membersQuery.isError ? (
          <ErrorBanner
            error={membersQuery.error}
            fallback="Project members failed to load"
            onRetry={() => void membersQuery.refetch()}
          />
        ) : null}
      </div>

      {linksQuery.isError ? null : isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-16 w-full" />
        </div>
      ) : links.length === 0 ? (
        <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-border py-12 text-center">
          <Robot size={28} className="mb-3 text-muted-foreground/50" />
          <p className="text-sm font-medium text-foreground">No linked agents</p>
          <p className="mt-1 text-sm text-muted-foreground">
            {canManage
              ? 'Link an agent to make it available to project members.'
              : 'No agents have been explicitly linked to this project.'}
          </p>
          {canManage ? (
            <Button className="mt-4" onClick={() => setLinkDialogOpen(true)}>
              <Plus size={14} className="mr-1.5" />
              Link Agent
            </Button>
          ) : null}
        </div>
      ) : (
        <div className="space-y-2">
          {links.map((link) => (
            <LinkedAgentRow
              key={link.id}
              link={link}
              agent={projectAgentsById.get(link.agent_id)}
              canManage={canManage}
              membersByUserId={membersByUserId}
              currentUser={currentUser}
              unlinkPending={deleteLink.isPending}
              pendingAgentId={deleteLink.variables}
              onUnlink={unlinkAgent}
            />
          ))}
        </div>
      )}

      {canManage ? (
        <LinkAgentDialog
          open={linkDialogOpen}
          onOpenChange={setLinkDialogOpen}
          projectId={projectId}
          linkedAgentIds={linkedAgentIds}
          membersByUserId={membersByUserId}
          currentUser={currentUser}
        />
      ) : null}
    </>
  )
}
