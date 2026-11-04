import { useEffect, useState } from 'react'
import { Plus, Trash } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useAgentsInfiniteQuery,
  useMembersQuery,
  useProjectQuery,
  useProjectTaskRoleDefaultsQuery,
  useUpdateProjectTaskRoleDefaults,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { settingsErrorMessage } from '@/components/settings/project-settings-utils'
import { useAuthStore } from '@/stores/auth'
import type { ActorRef, CoordinationMode, ProjectTaskRoleDefault } from '@/types/generated'

const COORDINATION_MODES: Array<{ value: CoordinationMode; label: string }> = [
  { value: 'independent', label: 'Independent' },
  { value: 'partitioned', label: 'Partitioned' },
  { value: 'collaborative', label: 'Collaborative' },
]

function actorKey(actor: ActorRef): string {
  return `${actor.kind}:${actor.id}`
}

function actorFromKey(key: string): ActorRef | undefined {
  const separator = key.indexOf(':')
  if (separator < 0) return undefined
  const kind = key.slice(0, separator)
  const id = key.slice(separator + 1)
  if (!id) return undefined
  if (kind === 'human') return { kind: 'human', id }
  if (kind === 'agent') return { kind: 'agent', id }
  return undefined
}

export function ProjectWorkersTab({ projectId }: { projectId: string }) {
  const user = useAuthStore((state) => state.user)
  const projectQuery = useProjectQuery(projectId)
  const membersQuery = useMembersQuery(projectId)
  const agentsQuery = useAgentsInfiniteQuery(100)
  const defaultsQuery = useProjectTaskRoleDefaultsQuery(projectId)
  const updateDefaults = useUpdateProjectTaskRoleDefaults()
  const [roles, setRoles] = useState<ProjectTaskRoleDefault[]>([])
  const [newRole, setNewRole] = useState('')
  const [selectedActors, setSelectedActors] = useState<Record<string, string>>({})

  useEffect(() => {
    if (!defaultsQuery.data) return
    setRoles(defaultsQuery.data.roles)
    setSelectedActors({})
  }, [defaultsQuery.data])

  const project = projectQuery.data
  const members = membersQuery.data ?? []
  const agents = (agentsQuery.data?.pages.flatMap((page) => page.items) ?? []).filter(
    (agent) => project?.owner_id != null && agent.owner_id === project.owner_id,
  )
  const currentMember = members.find((member) => member.user_id === user?.id)
  const canManage =
    currentMember?.role === 'admin' ||
    (currentMember?.role === 'owner' && project?.owner_id === user?.id)
  const defaults = defaultsQuery.data
  const isDirty = JSON.stringify(roles) !== JSON.stringify(defaults?.roles ?? [])
  const isLoading =
    projectQuery.isLoading ||
    membersQuery.isLoading ||
    defaultsQuery.isLoading ||
    agentsQuery.isLoading

  const updateRole = (
    roleName: string,
    update: (role: ProjectTaskRoleDefault) => ProjectTaskRoleDefault,
  ) => {
    setRoles((current) => current.map((role) => (role.role === roleName ? update(role) : role)))
  }

  const addRole = () => {
    const role = newRole.trim().toLowerCase()
    if (!role) return
    if (roles.some((item) => item.role === role)) {
      toast.error('That role already has Project defaults')
      return
    }
    setRoles((current) => [...current, { role, coordination_mode: 'independent', members: [] }])
    setNewRole('')
  }

  const addActor = (role: ProjectTaskRoleDefault) => {
    const actor = actorFromKey(selectedActors[role.role] ?? '')
    if (!actor || role.members.some((member) => actorKey(member) === actorKey(actor))) return
    updateRole(role.role, (current) => ({ ...current, members: [...current.members, actor] }))
    setSelectedActors((current) => ({ ...current, [role.role]: '' }))
  }

  const save = () => {
    if (!defaults || !canManage || !isDirty || updateDefaults.isPending) return
    updateDefaults.mutate(
      {
        projectId,
        body: {
          expected_version: defaults.version,
          roles,
        },
      },
      {
        onError: (error) =>
          toast.error(settingsErrorMessage(error, 'Project workers could not be saved')),
        onSuccess: () => toast.success('Default workers saved'),
      },
    )
  }

  return (
    <div className="max-w-3xl">
      <div className="mb-6 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-page font-semibold tracking-tight">Default workers</h2>
          <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
            Choose the Humans and Agents assigned to each role on new Tasks. Each Task receives a
            copy that can be changed independently.
          </p>
        </div>
        <Button disabled={!canManage || !isDirty || updateDefaults.isPending} onClick={save}>
          {updateDefaults.isPending ? 'Saving…' : 'Save defaults'}
        </Button>
      </div>

      {isLoading ? (
        <p className="py-6 text-sm text-muted-foreground">Loading Project workers…</p>
      ) : null}
      {defaultsQuery.isError ? (
        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {settingsErrorMessage(defaultsQuery.error, 'Project workers could not be loaded')}
          <Button
            className="ml-2"
            size="sm"
            variant="outline"
            onClick={() => void defaultsQuery.refetch()}
          >
            Retry
          </Button>
        </div>
      ) : null}
      {membersQuery.isError ? (
        <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {settingsErrorMessage(membersQuery.error, 'Project members could not be loaded')}
          <Button className="ml-2" size="sm" variant="outline" onClick={() => void membersQuery.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      {agentsQuery.isError ? (
        <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {settingsErrorMessage(agentsQuery.error, 'Agents could not be loaded')}
          <Button className="ml-2" size="sm" variant="outline" onClick={() => void agentsQuery.refetch()}>
            Retry
          </Button>
        </div>
      ) : null}
      {!isLoading && !canManage ? (
        <p className="mb-4 rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
          Project owner or admin membership is required to change default workers.
        </p>
      ) : null}
      {project?.owner_id == null ? (
        <p className="mb-4 rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
          Agents can be defaults only when they are owned by this Project’s canonical owner. This
          ownerless Project can still use Human defaults.
        </p>
      ) : agents.length === 0 && !agentsQuery.hasNextPage ? (
        <p className="mb-4 rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
          No Agents owned by the Project owner are available to this account. Global visibility
          alone does not authorize a Project to run another account’s Agent.
        </p>
      ) : null}

      <div className="space-y-3">
        {roles.map((role) => {
          const selected = selectedActors[role.role] ?? ''
          const availableMembers = members.filter(
            (member) =>
              !role.members.some((actor) => actor.kind === 'human' && actor.id === member.user_id),
          )
          const availableAgents = agents.filter(
            (agent) =>
              !role.members.some((actor) => actor.kind === 'agent' && actor.id === agent.id),
          )
          return (
            <section key={role.role} className="rounded-lg border border-border-subtle bg-card p-4">
              <div className="flex flex-wrap items-center gap-3">
                <div className="min-w-32 flex-1">
                  <p className="font-mono text-sm font-semibold text-foreground">{role.role}</p>
                  <p className="text-xs text-muted-foreground">Task role</p>
                </div>
                <label className="flex items-center gap-2 text-xs text-muted-foreground">
                  Coordination
                  <select
                    className="h-8 rounded-md border border-input bg-background px-2 text-sm text-foreground"
                    disabled={!canManage}
                    value={role.coordination_mode}
                    onChange={(event) =>
                      updateRole(role.role, (current) => ({
                        ...current,
                        coordination_mode: event.target.value as CoordinationMode,
                      }))
                    }
                  >
                    {COORDINATION_MODES.map((mode) => (
                      <option key={mode.value} value={mode.value}>
                        {mode.label}
                      </option>
                    ))}
                  </select>
                </label>
                <Button
                  aria-label={`Remove ${role.role} default role`}
                  disabled={!canManage}
                  size="icon-sm"
                  variant="ghost"
                  onClick={() =>
                    setRoles((current) => current.filter((item) => item.role !== role.role))
                  }
                >
                  <Trash size={15} />
                </Button>
              </div>

              <div className="mt-3 flex flex-wrap gap-2">
                {role.members.map((actor) => {
                  const member =
                    actor.kind === 'human'
                      ? members.find((item) => item.user_id === actor.id)
                      : undefined
                  const agent =
                    actor.kind === 'agent' ? agents.find((item) => item.id === actor.id) : undefined
                  const label =
                    actor.kind === 'human'
                      ? member?.display_name || member?.email || actor.id
                      : agent?.name || actor.id
                  return (
                    <span
                      key={actorKey(actor)}
                      className="inline-flex items-center gap-1.5 rounded-full border bg-background px-2.5 py-1 text-xs"
                    >
                      <span className="text-muted-foreground">
                        {actor.kind === 'human' ? 'Human' : 'Agent'}
                      </span>
                      <span className="max-w-48 truncate">{label}</span>
                      <button
                        type="button"
                        aria-label={`Remove ${label} from ${role.role}`}
                        className="ml-0.5 text-muted-foreground hover:text-foreground disabled:opacity-50"
                        disabled={!canManage}
                        onClick={() =>
                          updateRole(role.role, (current) => ({
                            ...current,
                            members: current.members.filter(
                              (item) => actorKey(item) !== actorKey(actor),
                            ),
                          }))
                        }
                      >
                        ×
                      </button>
                    </span>
                  )
                })}
                {role.members.length === 0 ? (
                  <span className="text-xs text-muted-foreground">No default members</span>
                ) : null}
              </div>

              <div className="mt-3 flex flex-wrap gap-2">
                <select
                  aria-label={`Add member to ${role.role}`}
                  className="h-8 min-w-56 rounded-md border border-input bg-background px-2 text-sm text-foreground"
                  disabled={!canManage || (!availableMembers.length && !availableAgents.length)}
                  value={selected}
                  onChange={(event) =>
                    setSelectedActors((current) => ({
                      ...current,
                      [role.role]: event.target.value,
                    }))
                  }
                >
                  <option value="">Select a Human or Agent…</option>
                  {availableMembers.length ? (
                    <optgroup label="Humans">
                      {availableMembers.map((member) => (
                        <option key={member.user_id} value={`human:${member.user_id}`}>
                          {member.display_name || member.email}
                        </option>
                      ))}
                    </optgroup>
                  ) : null}
                  {availableAgents.length ? (
                    <optgroup label="Agents">
                      {availableAgents.map((agent) => (
                        <option key={agent.id} value={`agent:${agent.id}`}>
                          {agent.name}
                        </option>
                      ))}
                    </optgroup>
                  ) : null}
                </select>
                <Button
                  disabled={!canManage || !selected}
                  size="sm"
                  variant="outline"
                  onClick={() => addActor(role)}
                >
                  <Plus size={14} /> Add worker
                </Button>
              </div>
            </section>
          )
        })}
      </div>

      <div className="mt-5 flex flex-wrap gap-2">
        <input
          aria-label="New Task role"
          className="h-9 min-w-56 rounded-md border border-input bg-background px-3 text-sm text-foreground"
          disabled={!canManage}
          placeholder="Role name, e.g. implementer"
          value={newRole}
          onChange={(event) => setNewRole(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault()
              addRole()
            }
          }}
        />
        <Button
          disabled={!canManage || !newRole.trim()}
          size="sm"
          variant="outline"
          onClick={addRole}
        >
          <Plus size={14} /> Add role
        </Button>
      </div>
      <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
        Removing a Human from Project membership or transferring Project ownership removes that
        Actor from these defaults. Existing Tasks keep their own role memberships.
      </p>
      {agentsQuery.hasNextPage ? (
        <Button
          className="mt-3"
          disabled={agentsQuery.isFetchingNextPage}
          size="sm"
          variant="outline"
          onClick={() => void agentsQuery.fetchNextPage()}
        >
          {agentsQuery.isFetchingNextPage ? 'Loading Agents…' : 'Load more Agents'}
        </Button>
      ) : null}
    </div>
  )
}
