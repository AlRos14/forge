import { useQuery } from '@tanstack/react-query'
import { apiFetch } from '@/api/client'
import type {
  ActorRef,
  ProjectTaskRoleConfiguration,
  ProjectTaskRoleDefault,
  ReworkActorSelection,
  TaskRoleCapability,
  TaskRoleCapabilityCatalog,
  TaskRoleConfiguration,
  TaskRoleResponse,
} from '@/types/generated'

const catalogKey = ['task-role-capability-catalog'] as const

function useCapabilityCatalog() {
  return useQuery({
    queryKey: catalogKey,
    queryFn: () => apiFetch<TaskRoleCapabilityCatalog>('/task-role-capabilities'),
    staleTime: 5 * 60 * 1000,
  })
}

function actorKey(actor: ActorRef): string {
  return `${actor.kind}:${actor.id}`
}

function parseActor(value: string): ActorRef | null {
  const separator = value.indexOf(':')
  if (separator < 1 || separator === value.length - 1) return null
  const kind = value.slice(0, separator)
  const id = value.slice(separator + 1)
  if (kind === 'human') return { kind, id }
  if (kind === 'agent') return { kind, id }
  return null
}

function actorLabel(actor: ActorRef): string {
  return `${actor.kind}:${actor.id}`
}

function selectionValue(selection: ReworkActorSelection | null): string {
  if (!selection) return ''
  return selection.kind === 'original_contributor'
    ? 'original_contributor'
    : actorKey(selection.actor_ref)
}

function selectionFromValue(value: string): ReworkActorSelection | null {
  if (!value) return null
  if (value === 'original_contributor') return { kind: 'original_contributor' }
  const actor = parseActor(value)
  return actor ? { kind: 'explicit', actor_ref: actor } : null
}

function ActorSelect({
  label,
  value,
  actors,
  allowOriginalContributor = false,
  onChange,
}: {
  label: string
  value: string
  actors: ActorRef[]
  allowOriginalContributor?: boolean
  onChange: (value: string) => void
}) {
  const uniqueActors = [...new Map(actors.map((actor) => [actorKey(actor), actor])).values()]
  return (
    <label className="space-y-1 text-xs">
      <span>{label}</span>
      <select
        className="h-9 w-full min-w-0 rounded-md border border-input bg-background px-2 text-sm text-foreground"
        value={value}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="">Choose an Actor</option>
        {allowOriginalContributor ? <option value="original_contributor">Sole original contributor</option> : null}
        {uniqueActors.map((actor) => (
          <option key={actorKey(actor)} value={actorKey(actor)}>
            {actorLabel(actor)}
          </option>
        ))}
      </select>
    </label>
  )
}

function CapabilityFields({
  capabilities,
  label,
  onChange,
}: {
  capabilities: TaskRoleCapability[]
  label: string | null
  onChange: (capabilities: TaskRoleCapability[], displayLabel: string | null) => void
}) {
  const catalog = useCapabilityCatalog()
  if (catalog.isLoading) return <p className="text-xs text-muted-foreground">Loading capabilities…</p>
  if (catalog.isError || !catalog.data) {
    return (
      <p role="alert" className="text-xs text-destructive">
        The capability catalog could not be loaded. Retry the Task to edit role authority.
      </p>
    )
  }
  return (
    <div className="space-y-3 rounded-md bg-muted/20 p-3">
      <label className="block max-w-lg space-y-1 text-xs">
        <span>Display label (optional)</span>
        <input
          className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground"
          maxLength={120}
          value={label ?? ''}
          onChange={(event) => onChange(capabilities, event.target.value || null)}
        />
      </label>
      <fieldset className="space-y-2">
        <legend className="text-xs font-medium">Capabilities</legend>
        {catalog.data.capabilities.map((capability) => (
          <label key={capability} className="flex min-w-0 items-start gap-2 text-xs">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={capabilities.includes(capability)}
              onChange={(event) => {
                const next = event.target.checked
                  ? [...capabilities, capability]
                  : capabilities.filter((item) => item !== capability)
                onChange(next, label)
              }}
            />
            <span className="min-w-0">
              <span className="block font-medium">{capability.replaceAll('_', ' ')}</span>
              <span className="text-muted-foreground">
                {capability === 'formal_review'
                  ? 'May produce a ReviewReport for an exact WorkUnit result.'
                  : capability === 'orchestration'
                    ? 'May use the currently supported typed orchestration operations.'
                    : 'May request a correction through the persisted route below.'}
              </span>
            </span>
          </label>
        ))}
      </fieldset>
      <p className="text-xs text-muted-foreground">
        Capabilities do not grant repository access or membership. Current TaskRole membership and
        the allocated WorkUnit still control eligibility and scope.
      </p>
    </div>
  )
}

export function TaskRoleConfigurationForm({
  role,
  roles,
  configuration,
  onChange,
}: {
  role: TaskRoleResponse
  roles: TaskRoleResponse[]
  configuration: TaskRoleConfiguration
  onChange: (configuration: TaskRoleConfiguration) => void
}) {
  const changeCapabilities = (capabilities: TaskRoleCapability[], displayLabel: string | null) => {
    const requestsRework = capabilities.includes('request_rework')
    onChange({
      ...configuration,
      capabilities,
      display_label: displayLabel,
      rework: requestsRework
        ? configuration.rework ?? {
            mode: 'manual',
            allowed_destination_role_ids: [],
            next_review_role_id: null,
            correction_actor: null,
            review_actor: null,
          }
        : null,
    })
  }
  const rework = configuration.rework
  const reviewerRoles = roles.filter((item) => item.configuration.capabilities.includes('formal_review'))
  const correctionActors = (rework?.allowed_destination_role_ids ?? []).flatMap(
    (id) => roles.find((item) => item.id === id)?.members.filter((member) => member.status === 'active').map((member) => member.actor_ref) ?? [],
  )
  const reviewActors = roles.find((item) => item.id === rework?.next_review_role_id)?.members
    .filter((member) => member.status === 'active')
    .map((member) => member.actor_ref) ?? []
  const correctionValue = selectionValue(rework?.correction_actor ?? null)
  const reviewValue = rework?.review_actor ? actorKey(rework.review_actor) : ''

  return (
    <div className="space-y-3">
      <CapabilityFields
        capabilities={configuration.capabilities}
        label={configuration.display_label}
        onChange={changeCapabilities}
      />
      {rework ? (
        <div className="space-y-3 rounded-md border p-3">
          <h4 className="text-sm font-medium">Correction route</h4>
          <label className="block max-w-sm space-y-1 text-xs">
            <span>Mode</span>
            <select
              className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm text-foreground"
              value={rework.mode}
              onChange={(event) => onChange({ ...configuration, rework: { ...rework, mode: event.target.value as typeof rework.mode } })}
            >
              <option value="manual">Manual request</option>
              <option value="automatic">Automatic after request_changes</option>
            </select>
          </label>
          <fieldset className="space-y-2">
            <legend className="text-xs font-medium">Allowed correction TaskRoles</legend>
            {roles.filter((item) => item.id !== role.id).map((item) => (
              <label key={item.id} className="flex items-center gap-2 text-xs">
                <input
                  type="checkbox"
                  checked={rework.allowed_destination_role_ids.includes(item.id)}
                  onChange={(event) => {
                    const allowed = new Set(rework.allowed_destination_role_ids)
                    if (event.target.checked) allowed.add(item.id)
                    else allowed.delete(item.id)
                    onChange({ ...configuration, rework: { ...rework, allowed_destination_role_ids: [...allowed] } })
                  }}
                />
                {item.configuration.display_label || item.role}
              </label>
            ))}
          </fieldset>
          <label className="block max-w-lg space-y-1 text-xs">
            <span>Next review TaskRole</span>
            <select
              className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm text-foreground"
              value={rework.next_review_role_id ?? ''}
              onChange={(event) => onChange({ ...configuration, rework: { ...rework, next_review_role_id: event.target.value || null, review_actor: null } })}
            >
              <option value="">Choose a formal review role</option>
              {reviewerRoles.map((item) => <option key={item.id} value={item.id}>{item.configuration.display_label || item.role}</option>)}
            </select>
          </label>
          <div className="grid gap-3 md:grid-cols-2">
            <ActorSelect
              label="Correction Actor selection"
              value={correctionValue}
              actors={correctionActors}
              allowOriginalContributor
              onChange={(value) => onChange({ ...configuration, rework: { ...rework, correction_actor: selectionFromValue(value) } })}
            />
            <ActorSelect
              label="Next review Actor"
              value={reviewValue}
              actors={reviewActors}
              onChange={(value) => onChange({ ...configuration, rework: { ...rework, review_actor: parseActor(value) } })}
            />
          </div>
          {rework.mode === 'automatic' && (!rework.allowed_destination_role_ids.length || !rework.next_review_role_id || !rework.correction_actor || !rework.review_actor) ? (
            <p className="text-xs text-destructive">Automatic mode requires a correction role, a formal review role, and both Actor selections.</p>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

export function ProjectTaskRoleConfigurationForm({
  roleName,
  roles,
  configuration,
  onChange,
}: {
  roleName: string
  roles: ProjectTaskRoleDefault[]
  configuration: ProjectTaskRoleConfiguration
  onChange: (configuration: ProjectTaskRoleConfiguration) => void
}) {
  const changeCapabilities = (capabilities: TaskRoleCapability[], displayLabel: string | null) => {
    const requestsRework = capabilities.includes('request_rework')
    onChange({
      ...configuration,
      capabilities,
      display_label: displayLabel,
      rework: requestsRework
        ? configuration.rework ?? {
            mode: 'manual',
            allowed_destination_roles: [],
            next_review_role: null,
            correction_actor: null,
            review_actor: null,
          }
        : null,
    })
  }
  const rework = configuration.rework
  const reviewerRoles = roles.filter((item) => item.configuration.capabilities.includes('formal_review'))
  const correctionActors = (rework?.allowed_destination_roles ?? []).flatMap(
    (name) => roles.find((item) => item.role === name)?.members ?? [],
  )
  const reviewActors = roles.find((item) => item.role === rework?.next_review_role)?.members ?? []
  const correctionValue = selectionValue(rework?.correction_actor ?? null)
  const reviewValue = rework?.review_actor ? actorKey(rework.review_actor) : ''

  return (
    <div className="space-y-3">
      <CapabilityFields
        capabilities={configuration.capabilities}
        label={configuration.display_label}
        onChange={changeCapabilities}
      />
      {rework ? (
        <div className="space-y-3 rounded-md border p-3">
          <h4 className="text-sm font-medium">Default correction route</h4>
          <label className="block max-w-sm space-y-1 text-xs">
            <span>Mode</span>
            <select className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm text-foreground" value={rework.mode} onChange={(event) => onChange({ ...configuration, rework: { ...rework, mode: event.target.value as typeof rework.mode } })}>
              <option value="manual">Manual request</option>
              <option value="automatic">Automatic after request_changes</option>
            </select>
          </label>
          <fieldset className="space-y-2">
            <legend className="text-xs font-medium">Allowed correction roles</legend>
            {roles.filter((item) => item.role !== roleName).map((item) => (
              <label key={item.role} className="flex items-center gap-2 text-xs">
                <input type="checkbox" checked={rework.allowed_destination_roles.includes(item.role)} onChange={(event) => {
                  const allowed = new Set(rework.allowed_destination_roles)
                  if (event.target.checked) allowed.add(item.role)
                  else allowed.delete(item.role)
                  onChange({ ...configuration, rework: { ...rework, allowed_destination_roles: [...allowed] } })
                }} />
                {item.configuration.display_label || item.role}
              </label>
            ))}
          </fieldset>
          <label className="block max-w-lg space-y-1 text-xs">
            <span>Next review role</span>
            <select className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm text-foreground" value={rework.next_review_role ?? ''} onChange={(event) => onChange({ ...configuration, rework: { ...rework, next_review_role: event.target.value || null, review_actor: null } })}>
              <option value="">Choose a formal review role</option>
              {reviewerRoles.map((item) => <option key={item.role} value={item.role}>{item.configuration.display_label || item.role}</option>)}
            </select>
          </label>
          <div className="grid gap-3 md:grid-cols-2">
            <ActorSelect label="Correction Actor selection" value={correctionValue} actors={correctionActors} allowOriginalContributor onChange={(value) => onChange({ ...configuration, rework: { ...rework, correction_actor: selectionFromValue(value) } })} />
            <ActorSelect label="Next review Actor" value={reviewValue} actors={reviewActors} onChange={(value) => onChange({ ...configuration, rework: { ...rework, review_actor: parseActor(value) } })} />
          </div>
          {rework.mode === 'automatic' && (!rework.allowed_destination_roles.length || !rework.next_review_role || !rework.correction_actor || !rework.review_actor) ? (
            <p className="text-xs text-destructive">Automatic mode requires a correction role, a formal review role, and both Actor selections.</p>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
