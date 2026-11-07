import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Plus, Trash } from '@phosphor-icons/react'
import { toast } from 'sonner'
import {
  useAgentsInfiniteQuery,
  useDaemonsQuery,
  useMembersQuery,
  useProjectQuery,
  useProjectTaskRoleDefaultsQuery,
  useUpdateProjectTaskRoleDefaults,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { CollapsibleSection } from '@/components/ui/collapsible-section'
import { apiFetch } from '@/api/client'
import { ProjectTaskRoleConfigurationForm } from '@/features/task-roles/TaskRoleConfigurationForm'
import { ActorIdentity, resolveActorIdentity } from '@/features/task-roles/ActorIdentity'
import { settingsErrorMessage } from '@/components/settings/project-settings-utils'
import { useAuthStore } from '@/stores/auth'
import type {
  ActorRef,
  CoordinationMode,
  ProjectTaskRoleConfiguration,
  TaskRoleCapabilityCatalog,
  ProjectTaskRoleDefault,
} from '@/types/generated'

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
  const daemonsQuery = useDaemonsQuery()
  const defaultsQuery = useProjectTaskRoleDefaultsQuery(projectId)
  const capabilityCatalog = useQuery({
    queryKey: ['task-role-capability-catalog'],
    queryFn: () => apiFetch<TaskRoleCapabilityCatalog>('/task-role-capabilities'),
    staleTime: 5 * 60 * 1000,
  })
  const updateDefaults = useUpdateProjectTaskRoleDefaults()
  const [roles, setRoles] = useState<ProjectTaskRoleDefault[]>([])
  const [newRole, setNewRole] = useState('')
  const [newRoleTemplate, setNewRoleTemplate] = useState('custom')
  const [selectedActors, setSelectedActors] = useState<Record<string, string>>({})

  useEffect(() => {
    if (!defaultsQuery.data) return
    setRoles(defaultsQuery.data.roles)
    setSelectedActors({})
  }, [defaultsQuery.data])

  const project = projectQuery.data
  const members = membersQuery.data ?? []
  const daemons = daemonsQuery.data?.items ?? []
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
    agentsQuery.isLoading ||
    daemonsQuery.isLoading

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
    const template = capabilityCatalog.data?.templates.find((item) => item.id === newRoleTemplate)
    setRoles((current) => [
      ...current,
      {
        role,
        coordination_mode: 'independent',
        configuration: {
          schema_version: 1,
          display_label: template?.default_role === 'custom' ? null : (template?.label ?? null),
          capabilities: template?.default_capabilities ?? [],
          rework: null,
        },
        members: [],
      },
    ])
    setNewRole('')
  }

  const addActor = (role: ProjectTaskRoleDefault) => {
    const actor = actorFromKey(selectedActors[role.role] ?? '')
    if (!actor || role.members.some((member) => actorKey(member) === actorKey(actor))) return
    updateRole(role.role, (current) => ({ ...current, members: [...current.members, actor] }))
    setSelectedActors((current) => ({ ...current, [role.role]: '' }))
  }

  const actorOptionLabel = (actor: ActorRef) => {
    const identity = resolveActorIdentity(actor, agents, members, daemons)
    return `${identity.name} · ${identity.context}`
  }
  const selectedTemplate = capabilityCatalog.data?.templates.find(
    (template) => template.id === newRoleTemplate,
  )

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
      <div className="mb-6 space-y-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs font-semibold uppercase tracking-wide text-primary">
              Project team
            </p>
            <h2 className="text-page font-semibold tracking-tight">Team &amp; workflow</h2>
            <p className="mt-1 max-w-2xl text-sm text-muted-foreground">
              Choose the people, Agents and responsibilities available to new Tasks. Each Task
              copies these defaults; later Project changes do not rewrite active work.
            </p>
          </div>
          <Button disabled={!canManage || !isDirty || updateDefaults.isPending} onClick={save}>
            {updateDefaults.isPending ? 'Saving…' : 'Save team defaults'}
          </Button>
        </div>
        <section
          className="rounded-lg border border-border-subtle bg-card p-4"
          aria-labelledby="add-project-role-title"
        >
          <div className="mb-3">
            <h3 id="add-project-role-title" className="font-semibold">
              Add role
            </h3>
            <p className="mt-1 text-sm text-muted-foreground">
              Start from a responsibility template, then choose eligible Project members.
            </p>
          </div>
          <div className="grid min-w-0 gap-3 sm:grid-cols-[minmax(12rem,0.8fr)_minmax(14rem,1fr)_auto] sm:items-end">
            <label className="min-w-0 space-y-1 text-xs font-medium">
              <span>Role template</span>
              <select
                aria-label="Project role template"
                className="h-10 w-full min-w-0 rounded-md border border-input bg-background px-3 text-sm text-foreground"
                disabled={!canManage || capabilityCatalog.isLoading}
                value={newRoleTemplate}
                onChange={(event) => {
                  const templateId = event.target.value
                  setNewRoleTemplate(templateId)
                  const template = capabilityCatalog.data?.templates.find(
                    (item) => item.id === templateId,
                  )
                  if (template?.default_role) setNewRole(template.default_role)
                }}
              >
                {(capabilityCatalog.data?.templates ?? []).map((template) => (
                  <option key={template.id} value={template.id}>
                    {template.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="min-w-0 space-y-1 text-xs font-medium">
              <span>Role name</span>
              <input
                aria-label="New Task role"
                className="h-10 w-full min-w-0 rounded-md border border-input bg-background px-3 text-sm text-foreground"
                disabled={!canManage}
                placeholder="e.g. implementer"
                value={newRole}
                onChange={(event) => setNewRole(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    addRole()
                  }
                }}
              />
            </label>
            <Button disabled={!canManage || !newRole.trim()} onClick={addRole}>
              <Plus size={15} aria-hidden="true" /> Add role
            </Button>
          </div>
          {selectedTemplate?.description ? (
            <p className="mt-3 max-w-3xl text-xs leading-relaxed text-muted-foreground">
              <span className="font-medium text-foreground">{selectedTemplate.label}:</span>{' '}
              {selectedTemplate.description}
            </p>
          ) : null}
        </section>
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
          <Button
            className="ml-2"
            size="sm"
            variant="outline"
            onClick={() => void membersQuery.refetch()}
          >
            Retry
          </Button>
        </div>
      ) : null}
      {agentsQuery.isError ? (
        <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
          {settingsErrorMessage(agentsQuery.error, 'Agents could not be loaded')}
          <Button
            className="ml-2"
            size="sm"
            variant="outline"
            onClick={() => void agentsQuery.refetch()}
          >
            Retry
          </Button>
        </div>
      ) : null}
      {daemonsQuery.isError ? (
        <div
          role="alert"
          className="mb-4 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive"
        >
          Host details could not be loaded. Agent names and harnesses remain available.
          <Button
            className="ml-2"
            size="sm"
            variant="outline"
            onClick={() => void daemonsQuery.refetch()}
          >
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
        {!isLoading && roles.length === 0 ? (
          <div className="rounded-lg border border-dashed p-6 text-center">
            <h3 className="font-semibold">No Project roles yet</h3>
            <p className="mx-auto mt-1 max-w-lg text-sm text-muted-foreground">
              Add a role above to save its responsibility and eligible members for new Tasks.
            </p>
          </div>
        ) : null}
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
            <section
              key={role.role}
              className="rounded-lg border border-border-subtle bg-card p-4 sm:p-5"
            >
              <div className="flex flex-wrap items-start gap-3">
                <div className="min-w-0 flex-1">
                  <h3 className="break-words text-base font-semibold text-foreground">
                    {role.configuration.display_label || role.role.replaceAll('_', ' ')}
                  </h3>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {role.configuration.capabilities.includes('orchestration')
                      ? 'Coordinates Task work under the configured policy.'
                      : role.configuration.capabilities.includes('formal_review')
                        ? 'Reviews an exact work result and records findings.'
                        : role.configuration.capabilities.includes('request_rework')
                          ? 'Can request a correction through the configured route.'
                          : 'Receives concrete work within this responsibility.'}
                  </p>
                  <details className="mt-1 text-xs text-muted-foreground">
                    <summary className="w-fit cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                      Technical role reference
                    </summary>
                    <code className="mt-1 block break-all">{role.role}</code>
                  </details>
                </div>
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

              <div className="mt-4 border-t pt-3">
                <CollapsibleSection
                  title="Advanced role settings"
                  badge={
                    COORDINATION_MODES.find((mode) => mode.value === role.coordination_mode)?.label
                  }
                >
                  <div className="mt-3 space-y-4 rounded-md bg-muted/20 p-3">
                    <label className="block max-w-sm space-y-1 text-xs">
                      <span className="font-medium">Coordination mode</span>
                      <select
                        className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm text-foreground"
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
                    <ProjectTaskRoleConfigurationForm
                      roleName={role.role}
                      roles={roles}
                      configuration={role.configuration}
                      actorLabel={actorOptionLabel}
                      disabled={!canManage}
                      onChange={(configuration: ProjectTaskRoleConfiguration) =>
                        updateRole(role.role, (current) => ({ ...current, configuration }))
                      }
                    />
                  </div>
                </CollapsibleSection>
              </div>

              <div className="mt-4 space-y-3 border-t pt-4">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <h4 className="text-sm font-medium">Eligible team members</h4>
                  <span className="text-xs text-muted-foreground">Copied to each new Task</span>
                </div>
                <div className="grid gap-2 sm:grid-cols-2">
                  {role.members.map((actor) => {
                    const label = resolveActorIdentity(actor, agents, members, daemons).name
                    return (
                      <div
                        key={actorKey(actor)}
                        className="flex min-w-0 items-start gap-2 rounded-md border bg-background p-2"
                      >
                        <ActorIdentity
                          actor={actor}
                          agents={agents}
                          members={members}
                          daemons={daemons}
                          className="min-w-0 flex-1"
                        />
                        <button
                          type="button"
                          aria-label={`Remove ${label} from ${role.role}`}
                          className="shrink-0 rounded p-1 text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
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
                      </div>
                    )
                  })}
                  {role.members.length === 0 ? (
                    <span className="text-xs text-muted-foreground">No default members</span>
                  ) : null}
                </div>
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
                          {member.display_name || 'Project member'} · {member.email}
                        </option>
                      ))}
                    </optgroup>
                  ) : null}
                  {availableAgents.length ? (
                    <optgroup label="Agents">
                      {availableAgents.map((agent) => (
                        <option key={agent.id} value={`agent:${agent.id}`}>
                          {agent.name} · {agent.executor_type.replaceAll('_', ' ')} ·{' '}
                          {daemons.find((daemon) => daemon.id === agent.daemon_id)?.hostname ??
                            'Host not pinned'}
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
                  <Plus size={14} /> Add member
                </Button>
              </div>
            </section>
          )
        })}
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
