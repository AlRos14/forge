import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { useQuery } from '@tanstack/react-query'
import { toast } from 'sonner'
import { apiFetch } from '@/api/client'
import {
  useAgentsQuery,
  useMembersQuery,
  useProjectQuery,
  useTaskGatesQuery,
  useTaskReviewLinesQuery,
  useWorkUnitArtifactPackageQuery,
} from '@/api/hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { TaskRoleConfigurationForm } from '@/features/task-roles/TaskRoleConfigurationForm'
import { getApiErrorMessage } from '@/lib/api-error'
import type {
  ActorRef,
  AddRoleMembershipRequest,
  AllocateWorkUnitRequest,
  CoordinationMode,
  CreateTaskRoleRequest,
  CreateWorkUnitRequest,
  Execution,
  ProjectMemberResponse,
  RoleMembershipResponse,
  RoleMembershipStatus,
  Task,
  TaskRoleResponse,
  TaskRoleCapabilityCatalog,
  TaskRoleConfiguration,
  UpdateRoleMembershipRequest,
  UpdateTaskRoleRequest,
  UpdateWorkUnitRequest,
  TransitionWorkUnitRequest,
  WorkUnitDependencyResponse,
  WorkUnitIntegrationRequest,
  WorkUnitReadinessResponse,
  WorkUnitResponse,
  WorkUnitStatus,
} from '@/types/generated'

type Props = {
  task: Task
  workUnits: WorkUnitResponse[]
  executions: Execution[]
  onChanged: () => Promise<unknown>
}

type RoleDraft = {
  coordinationMode: CoordinationMode
  policy: string
  configuration: TaskRoleConfiguration
}
type UnitDraft = { title: string; scope: string; executionInstructions: string; requiresIntegration: boolean }

const COORDINATION_MODES: CoordinationMode[] = ['partitioned', 'collaborative', 'independent']
const MEMBERSHIP_STATUSES: RoleMembershipStatus[] = ['active', 'suspended', 'ended']
const WORK_UNIT_STATUSES: WorkUnitStatus[] = ['open', 'completed', 'cancelled']
const capabilityCatalogQueryKey = ['task-role-capability-catalog'] as const

function hasFormalReview(role: TaskRoleResponse | undefined): boolean {
  return role?.configuration.capabilities.includes('formal_review') ?? false
}

function actorKey(actor: ActorRef | null): string {
  return actor ? `${actor.kind}:${actor.id}` : ''
}

function parseActorKey(value: string): ActorRef | null {
  if (!value) return null
  const [kind, ...id] = value.split(':')
  if ((kind !== 'agent' && kind !== 'human') || id.length === 0) return null
  return { kind, id: id.join(':') }
}

function actorLabel(
  actor: ActorRef,
  agents: Array<{ id: string; name: string }>,
  members: ProjectMemberResponse[],
) {
  if (actor.kind === 'agent') {
    const agent = agents.find((item) => item.id === actor.id)
    return `Agent ${agent?.name ?? actor.id}`
  }
  const member = members.find((item) => item.user_id === actor.id)
  return `Human ${member?.display_name || member?.email || actor.id}`
}

function actorOptions(
  role: TaskRoleResponse | undefined,
  agents: Array<{ id: string; name: string }>,
  members: ProjectMemberResponse[],
) {
  return (role?.members ?? [])
    .filter((member) => member.status === 'active')
    .map((member) => ({
      key: actorKey(member.actor_ref),
      label: actorLabel(member.actor_ref, agents, members),
    }))
}

async function postOrPatch<T>(
  path: string,
  method: 'POST' | 'PATCH' | 'DELETE',
  body: unknown,
): Promise<T> {
  return apiFetch<T>(path, { method, body: JSON.stringify(body) })
}

export function TaskWorkManagement({ task, workUnits, executions, onChanged }: Props) {
  const capabilityCatalog = useQuery({
    queryKey: capabilityCatalogQueryKey,
    queryFn: () => apiFetch<TaskRoleCapabilityCatalog>('/task-role-capabilities'),
    staleTime: 5 * 60 * 1000,
  })
  const projectQuery = useProjectQuery(task.project_id)
  const agentsQuery = useAgentsQuery()
  const membersQuery = useMembersQuery(task.project_id)
  const gatesQuery = useTaskGatesQuery(task.id)
  const reviewLinesQuery = useTaskReviewLinesQuery(task.id)
  const projectOwnerId = projectQuery.data?.owner_id
  // Global visibility is discovery only. A Task Agent must belong to this
  // Project's canonical owner account to pass current execution authority.
  const authorizedAgents = useMemo(
    () =>
      (agentsQuery.data?.items ?? []).filter((agent) =>
        Boolean(projectOwnerId && agent.owner_id === projectOwnerId),
      ),
    [agentsQuery.data?.items, projectOwnerId],
  )
  const projectMembers = membersQuery.data ?? []
  const rolePath = `/tasks/${task.id}/task-roles`
  const [newRole, setNewRole] = useState('implementer')
  const [newRoleTemplate, setNewRoleTemplate] = useState('implementer')
  const [newRoleMode, setNewRoleMode] = useState<CoordinationMode>('collaborative')
  const [roleDrafts, setRoleDrafts] = useState<Record<string, RoleDraft>>({})
  const [memberActors, setMemberActors] = useState<Record<string, string>>({})
  const [unitDrafts, setUnitDrafts] = useState<Record<string, UnitDraft>>({})
  const [allocations, setAllocations] = useState<Record<string, { role: string; actor: string }>>(
    {},
  )
  const [dependencyTargets, setDependencyTargets] = useState<Record<string, string>>({})
  const [freshReadiness, setFreshReadiness] = useState<Record<string, WorkUnitReadinessResponse>>(
    {},
  )
  const [creation, setCreation] = useState({
    title: '',
    scope: '',
    executionInstructions: '',
    role: '',
    parentId: '',
    assignedActor: '',
    reviewedWorkUnitId: '',
    artifactInputIds: '',
    approvalGateId: '',
    approvalLineId: '',
    requiresIntegration: false,
    provenanceKind: '',
    provenanceRef: '',
    provenanceActor: '',
  })

  useEffect(() => {
    setRoleDrafts(
      Object.fromEntries(
        task.task_roles.map((role) => [
          role.role,
          {
            coordinationMode: role.coordination_mode ?? 'collaborative',
            policy: JSON.stringify(role.policy, null, 2),
            configuration: role.configuration,
          },
        ]),
      ),
    )
  }, [task.task_roles])

  useEffect(() => {
    setUnitDrafts(
      Object.fromEntries(
        workUnits.map((unit) => [
          unit.id,
          {
            title: unit.title,
            scope: unit.scope,
            executionInstructions: unit.execution_instructions ?? '',
            requiresIntegration: unit.requires_integration,
          },
        ]),
      ),
    )
    setAllocations(
      Object.fromEntries(
        workUnits.map((unit) => [
          unit.id,
          {
            role: unit.role,
            actor: actorKey(unit.assigned_actor),
          },
        ]),
      ),
    )
    setFreshReadiness({})
  }, [workUnits])

  const createRole = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const body: CreateTaskRoleRequest = {
      role: newRole.trim(),
      coordination_mode: newRoleMode,
      configuration: {
        schema_version: 1,
        display_label: null,
        capabilities:
          capabilityCatalog.data?.templates.find((template) => template.id === newRoleTemplate)
            ?.default_capabilities ?? [],
        rework: null,
      },
      policy: {},
    }
    try {
      await postOrPatch<TaskRoleResponse>(rolePath, 'POST', body)
      toast.success('TaskRole created')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not create TaskRole'))
    }
  }

  const saveRole = async (role: TaskRoleResponse) => {
    const draft = roleDrafts[role.role]
    if (!draft) return
    let policy: Record<string, unknown>
    try {
      const parsed: unknown = JSON.parse(draft.policy)
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed))
        throw new Error('Policy must be a JSON object')
      policy = parsed as Record<string, unknown>
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'TaskRole policy must be valid JSON')
      return
    }
    const body: UpdateTaskRoleRequest = {
      expected_version: role.version,
      coordination_mode: draft.coordinationMode,
      configuration: draft.configuration,
      policy,
    }
    try {
      await postOrPatch<TaskRoleResponse>(
        `${rolePath}/${encodeURIComponent(role.role)}`,
        'PATCH',
        body,
      )
      toast.success(`TaskRole ${role.role} updated`)
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, `Could not update ${role.role}`))
    }
  }

  const addMember = async (event: FormEvent<HTMLFormElement>, role: TaskRoleResponse) => {
    event.preventDefault()
    const actor = parseActorKey(memberActors[role.role] ?? '')
    if (!actor) return
    const body: AddRoleMembershipRequest = { actor_ref: actor }
    try {
      await postOrPatch<RoleMembershipResponse>(
        `${rolePath}/${encodeURIComponent(role.role)}/members`,
        'POST',
        body,
      )
      setMemberActors((current) => ({ ...current, [role.role]: '' }))
      toast.success(`Member added to ${role.role}`)
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not add TaskRole member'))
    }
  }

  const updateMember = async (
    role: TaskRoleResponse,
    member: RoleMembershipResponse,
    status: RoleMembershipStatus,
  ) => {
    const body: UpdateRoleMembershipRequest = { expected_version: member.version, status }
    try {
      await postOrPatch<RoleMembershipResponse>(
        `${rolePath}/${encodeURIComponent(role.role)}/members/${member.id}`,
        'PATCH',
        body,
      )
      toast.success('TaskRole membership updated')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not update membership'))
    }
  }

  const createWorkUnit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const role = creation.role || task.task_roles[0]?.role || ''
    if (!creation.title.trim() || !creation.scope.trim() || !role) {
      toast.error('Title, scope, and an existing TaskRole are required')
      return
    }
    const formalReviewRole = hasFormalReview(task.task_roles.find((item) => item.role === role))
    if (formalReviewRole && !creation.reviewedWorkUnitId) {
      toast.error('Choose the exact WorkUnit this reviewer will inspect')
      return
    }
    if (Boolean(creation.approvalGateId) !== Boolean(creation.approvalLineId)) {
      toast.error('Choose both a Gate and a review line to require approval')
      return
    }
    const provenanceActor = parseActorKey(creation.provenanceActor)
    const provenance =
      creation.provenanceKind === 'actor'
        ? provenanceActor
          ? { kind: 'actor' as const, actor: provenanceActor }
          : null
        : creation.provenanceKind === 'work_unit'
          ? { kind: 'work_unit' as const, id: creation.provenanceRef.trim() }
          : creation.provenanceKind === 'artifact'
            ? { kind: 'artifact' as const, id: creation.provenanceRef.trim() }
            : creation.provenanceKind === 'external'
              ? { kind: 'external' as const, id: creation.provenanceRef.trim() }
              : null
    if (creation.provenanceKind === 'actor' && !provenanceActor) {
      toast.error('Choose an Actor for the WorkUnit provenance')
      return
    }
    if (provenance && provenance.kind !== 'actor' && !provenance.id) {
      toast.error('Enter a provenance reference')
      return
    }
    const body: CreateWorkUnitRequest = {
      title: creation.title.trim(),
      scope: creation.scope.trim(),
      execution_instructions: creation.executionInstructions.trim() || null,
      role,
      parent_work_unit_id: creation.parentId || null,
      assigned_actor: parseActorKey(creation.assignedActor),
      requires_integration: creation.requiresIntegration,
      provenance,
      artifact_inputs: [...new Set(creation.artifactInputIds.split(/[\n,]/).map((id) => id.trim()).filter(Boolean))],
      review_approval: creation.approvalGateId && creation.approvalLineId
        ? { gate_id: creation.approvalGateId, review_line_id: creation.approvalLineId }
        : null,
      ...(formalReviewRole
        ? { review_subject: { reviewed_work_unit_id: creation.reviewedWorkUnitId } }
        : {}),
    }
    try {
      await postOrPatch<WorkUnitResponse>(`/tasks/${task.id}/work-units`, 'POST', body)
      toast.success('WorkUnit created')
      setCreation({
        title: '',
        scope: '',
        executionInstructions: '',
        role,
        parentId: '',
        assignedActor: '',
        reviewedWorkUnitId: '',
        artifactInputIds: '',
        approvalGateId: '',
        approvalLineId: '',
        requiresIntegration: false,
        provenanceKind: '',
        provenanceRef: '',
        provenanceActor: '',
      })
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not create WorkUnit'))
    }
  }

  const saveWorkUnit = async (unit: WorkUnitResponse) => {
    const draft = unitDrafts[unit.id]
    if (!draft) return
    const body: UpdateWorkUnitRequest = {
      expected_version: unit.version,
      title: draft.title.trim() || null,
      scope: draft.scope.trim() || null,
      execution_instructions: draft.executionInstructions.trim() || null,
      requires_integration: draft.requiresIntegration,
    }
    try {
      await postOrPatch<WorkUnitResponse>(`/work-units/${unit.id}`, 'PATCH', body)
      toast.success('WorkUnit details updated')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not update WorkUnit'))
    }
  }

  const allocateWorkUnit = async (unit: WorkUnitResponse) => {
    const allocation = allocations[unit.id]
    if (!allocation?.role) return
    const reviewSubject = unit.review_subject
    const formalReviewRole = hasFormalReview(
      task.task_roles.find((item) => item.role === allocation.role),
    )
    if (formalReviewRole && !reviewSubject) {
      toast.error('Create a new reviewer WorkUnit with an exact subject instead')
      return
    }
    if (reviewSubject && !formalReviewRole) {
      toast.error('The role cannot change after an exact reviewer subject is assigned')
      return
    }
    const body: AllocateWorkUnitRequest = {
      expected_version: unit.version,
      role: allocation.role,
      assigned_actor: parseActorKey(allocation.actor),
    }
    try {
      await postOrPatch<WorkUnitResponse>(`/work-units/${unit.id}/allocation`, 'POST', body)
      toast.success('WorkUnit allocation updated')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not allocate WorkUnit'))
    }
  }

  const transitionWorkUnit = async (unit: WorkUnitResponse, status: WorkUnitStatus) => {
    const body: TransitionWorkUnitRequest = { expected_version: unit.version, status }
    try {
      await postOrPatch<WorkUnitResponse>(`/work-units/${unit.id}/status`, 'POST', body)
      toast.success(`WorkUnit moved to ${status}`)
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, `Could not move WorkUnit to ${status}`))
    }
  }

  const createDependency = async (unit: WorkUnitResponse) => {
    const prerequisiteId = dependencyTargets[unit.id]
    if (!prerequisiteId) return
    try {
      await postOrPatch<WorkUnitDependencyResponse>(
        `/work-units/${unit.id}/dependencies/${prerequisiteId}`,
        'POST',
        { expected_version: unit.version },
      )
      toast.success('WorkUnit dependency added')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not add dependency'))
    }
  }

  const removeDependency = async (unit: WorkUnitResponse, prerequisiteId: string) => {
    try {
      await postOrPatch<void>(`/work-units/${unit.id}/dependencies/${prerequisiteId}`, 'DELETE', {
        expected_version: unit.version,
      })
      toast.success('WorkUnit dependency removed')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not remove dependency'))
    }
  }

  const prepareWorkspace = async (unit: WorkUnitResponse) => {
    try {
      await postOrPatch(`/work-units/${unit.id}/workspace`, 'POST', {})
      toast.success('WorkUnit Workspace prepared')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not prepare WorkUnit Workspace'))
    }
  }

  const integrateWorkUnit = async (unit: WorkUnitResponse, executionId: string) => {    if (!executionId) return
    const body: WorkUnitIntegrationRequest = {
      execution_id: executionId,
      idempotency_key: crypto.randomUUID(),
    }
    try {
      await postOrPatch(`/work-units/${unit.id}/integrations`, 'POST', body)
      toast.success('WorkUnit integration requested')
      await onChanged()
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not integrate WorkUnit'))
    }
  }

  const refreshReadiness = async (unit: WorkUnitResponse) => {
    try {
      const readiness = await apiFetch<WorkUnitReadinessResponse>(
        `/work-units/${unit.id}/readiness`,
      )
      setFreshReadiness((current) => ({ ...current, [unit.id]: readiness }))
    } catch (error) {
      toast.error(getApiErrorMessage(error, 'Could not refresh WorkUnit readiness'))
    }
  }

  const eligibleActors = (roleName: string) =>
    actorOptions(
      task.task_roles.find((role) => role.role === roleName),
      authorizedAgents,
      projectMembers,
    )

  return (
    <section className="space-y-5 rounded-lg border p-4 lg:col-span-2">
      <div>
        <h2 className="font-semibold">Task assignments and WorkUnits</h2>
        <p className="mt-1 text-xs text-muted-foreground">
          TaskRole membership makes an Actor eligible. Each WorkUnit selects its exact role and
          Actor. Global Agent visibility alone does not authorize execution; only Agents owned by
          this Project&apos;s owner are offered here.
        </p>
      </div>
      {projectQuery.isError ? (
        <p role="alert" className="text-sm text-destructive">
          {getApiErrorMessage(projectQuery.error, 'Could not load Project authorization scope')}
        </p>
      ) : null}
      {agentsQuery.isError ? (
        <p role="alert" className="text-sm text-destructive">
          {getApiErrorMessage(agentsQuery.error, 'Could not load eligible Agents')}
        </p>
      ) : null}
      {membersQuery.isError ? (
        <p role="alert" className="text-sm text-destructive">
          {getApiErrorMessage(membersQuery.error, 'Could not load Project members')}
        </p>
      ) : null}

      <form
        onSubmit={(event) => void createRole(event)}
        className="grid gap-2 rounded-md border p-3 sm:grid-cols-[1fr_180px_auto]"
      >
        <label className="space-y-1 text-xs sm:col-span-3">
          <span>Role template</span>
          <select
            aria-label="TaskRole template"
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={newRoleTemplate}
            onChange={(event) => {
              const templateId = event.target.value
              setNewRoleTemplate(templateId)
              const template = capabilityCatalog.data?.templates.find((item) => item.id === templateId)
              if (template?.default_role) setNewRole(template.default_role)
            }}
          >
            {(capabilityCatalog.data?.templates ?? []).map((template) => (
              <option key={template.id} value={template.id}>{template.label}</option>
            ))}
          </select>
          <span className="block text-muted-foreground">
            Templates only prefill editable configuration. Capabilities never follow from the role name.
          </span>
        </label>
        <Input
          aria-label="New TaskRole name"
          value={newRole}
          onChange={(event) => setNewRole(event.target.value)}
          placeholder="implementer"
        />
        <select
          aria-label="New TaskRole coordination mode"
          className="h-9 rounded-md border bg-background px-2 text-sm"
          value={newRoleMode}
          onChange={(event) => setNewRoleMode(event.target.value as CoordinationMode)}
        >
          {COORDINATION_MODES.map((mode) => (
            <option key={mode} value={mode}>
              {mode}
            </option>
          ))}
        </select>
        <Button type="submit" disabled={!newRole.trim()}>
          Create TaskRole
        </Button>
      </form>

      {task.task_roles.map((role) => {
        const draft = roleDrafts[role.role]
        const alreadyCurrent = new Set(
          role.members
            .filter((member) => member.status !== 'ended')
            .map((member) => actorKey(member.actor_ref)),
        )
        const selectableActors = [
          ...authorizedAgents.map((agent) => ({
            value: `agent:${agent.id}`,
            label: `Agent ${agent.name}`,
          })),
          ...projectMembers.map((member) => ({
            value: `human:${member.user_id}`,
            label: `Human ${member.display_name || member.email}`,
          })),
        ].filter((option) => !alreadyCurrent.has(option.value))
        return (
          <article key={role.id} className="space-y-3 rounded-md border p-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h3 className="font-medium">{role.role}</h3>
                <p className="text-xs text-muted-foreground">TaskRole v{role.version}</p>
              </div>
              <Button
                size="sm"
                variant="outline"
                onClick={() => void saveRole(role)}
                disabled={!draft}
              >
                Save role settings
              </Button>
            </div>
            {draft ? (
              <div className="grid gap-3 md:grid-cols-2">
                <label className="space-y-1 text-xs">
                  <span>Coordination mode</span>
                  <select
                    className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                    value={draft.coordinationMode}
                    onChange={(event) =>
                      setRoleDrafts((current) => ({
                        ...current,
                        [role.role]: {
                          ...draft,
                          coordinationMode: event.target.value as CoordinationMode,
                        },
                      }))
                    }
                  >
                    {COORDINATION_MODES.map((mode) => (
                      <option key={mode} value={mode}>
                        {mode}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="space-y-1 text-xs">
                  <span>Policy JSON</span>
                  <Textarea
                    aria-label={`${role.role} policy JSON`}
                    className="min-h-24 font-mono text-xs"
                    value={draft.policy}
                    onChange={(event) =>
                      setRoleDrafts((current) => ({
                        ...current,
                        [role.role]: { ...draft, policy: event.target.value },
                      }))
                    }
                  />
                </label>
              </div>
            ) : null}
            {draft ? (
              <TaskRoleConfigurationForm
                role={role}
                roles={task.task_roles}
                configuration={draft.configuration}
                onChange={(configuration) =>
                  setRoleDrafts((current) => ({
                    ...current,
                    [role.role]: { ...draft, configuration },
                  }))
                }
              />
            ) : null}
            <div className="space-y-2">
              <h4 className="text-sm font-medium">Members</h4>
              {role.members.map((member) => (
                <div
                  key={member.id}
                  className="flex flex-wrap items-center gap-2 rounded bg-muted/30 p-2 text-xs"
                >
                  <span className="min-w-0 flex-1">
                    {actorLabel(member.actor_ref, authorizedAgents, projectMembers)} · v
                    {member.version}
                  </span>
                  <select
                    aria-label={`Membership status ${member.id}`}
                    className="h-8 rounded border bg-background px-2"
                    value={member.status}
                    onChange={(event) =>
                      void updateMember(role, member, event.target.value as RoleMembershipStatus)
                    }
                  >
                    {MEMBERSHIP_STATUSES.map((status) => (
                      <option key={status} value={status}>
                        {status}
                      </option>
                    ))}
                  </select>
                </div>
              ))}
              <form
                onSubmit={(event) => void addMember(event, role)}
                className="flex flex-wrap gap-2"
              >
                <select
                  aria-label={`Add member to ${role.role}`}
                  className="h-9 min-w-56 flex-1 rounded-md border bg-background px-2 text-sm"
                  value={memberActors[role.role] ?? ''}
                  onChange={(event) =>
                    setMemberActors((current) => ({ ...current, [role.role]: event.target.value }))
                  }
                >
                  <option value="">Choose an authorized Agent or Project member</option>
                  {selectableActors.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
                <Button
                  type="submit"
                  size="sm"
                  variant="outline"
                  disabled={!memberActors[role.role]}
                >
                  Add member
                </Button>
              </form>
              {authorizedAgents.length === 0 && !agentsQuery.isLoading ? (
                <p className="text-xs text-muted-foreground">
                  No Agent owned by the Project owner is available. Global Agents from other
                  accounts remain discoverable but cannot be authorized to execute here.
                </p>
              ) : null}
            </div>
          </article>
        )
      })}

      <div className="space-y-3 border-t pt-4">
        <h3 className="font-medium">Create a WorkUnit</h3>
        <p className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
          Forge WorkUnits are executable scopes for this Task. Enable a repository Workspace when the
          Agent needs to read this Task&apos;s repository. Forge prepares an isolated Workspace when
          the WorkUnit is prepared for launch; integrating its result into the Task branch is a
          separate explicit action. Optional execution instructions are passed to the assigned
          harness; they are not a second Task or Execution lifecycle.
        </p>
        <form
          onSubmit={(event) => void createWorkUnit(event)}
          className="grid gap-3 md:grid-cols-2"
        >
          <label className="space-y-1 text-xs">
            <span>Title</span>
            <Input
              value={creation.title}
              onChange={(event) =>
                setCreation((current) => ({ ...current, title: event.target.value }))
              }
            />
          </label>
          <label className="space-y-1 text-xs">
            <span>TaskRole</span>
            <select
              className="h-9 rounded-md border bg-background px-2 text-sm"
              value={creation.role || task.task_roles[0]?.role || ''}
              onChange={(event) => setCreation((current) => ({
                ...current,
                role: event.target.value,
                reviewedWorkUnitId: '',
              }))}
            >
              <option value="">Create a TaskRole first</option>
              {task.task_roles.map((role) => (
                <option key={role.id} value={role.role}>
                  {role.role}
                </option>
              ))}
            </select>
          </label>
          {hasFormalReview(
            task.task_roles.find((role) => role.role === (creation.role || task.task_roles[0]?.role)),
          ) ? (
            <label className="space-y-1 text-xs md:col-span-2">
              <span>Exact WorkUnit to review</span>
              <select
                aria-label="Exact WorkUnit to review"
                required
                className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                value={creation.reviewedWorkUnitId}
                onChange={(event) => setCreation((current) => ({
                  ...current,
                  reviewedWorkUnitId: event.target.value,
                }))}
              >
                <option value="">Choose the WorkUnit under review</option>
                {workUnits
                  .filter(
                    (unit) =>
                      !hasFormalReview(task.task_roles.find((role) => role.role === unit.role)) &&
                      unit.status !== 'cancelled',
                  )
                  .map((unit) => (
                    <option key={unit.id} value={unit.id}>
                      {unit.title} · {unit.status}
                      {unit.status === 'open' ? ' · waiting for its exact result' : ''}
                    </option>
                  ))}
              </select>
              <span className="block text-muted-foreground">
                Forge freezes the subject to that WorkUnit&apos;s exact result before review can run.
              </span>
            </label>
          ) : null}
          <label className="space-y-1 text-xs md:col-span-2">
            <span>Scope</span>
            <Textarea
              value={creation.scope}
              onChange={(event) =>
                setCreation((current) => ({ ...current, scope: event.target.value }))
              }
            />
          </label>
          <label className="space-y-1 text-xs md:col-span-2">
            <span>Harness execution instructions (optional)</span>
            <Textarea
              value={creation.executionInstructions}
              onChange={(event) =>
                setCreation((current) => ({
                  ...current,
                  executionInstructions: event.target.value,
                }))
              }
              placeholder="Exact instructions passed to the assigned harness instead of the generic Task/WorkUnit prompt"
            />
          </label>
          <label className="space-y-1 text-xs md:col-span-2">
            <span>Exact Artifact inputs (optional, one ID per line)</span>
            <Textarea
              value={creation.artifactInputIds}
              onChange={(event) =>
                setCreation((current) => ({ ...current, artifactInputIds: event.target.value }))
              }
              placeholder="Copy Artifact IDs from this Task&apos;s Artifacts tab"
            />
            <span className="block text-muted-foreground">
              Forge pins each selected Artifact, digest, and producer Execution. The WorkUnit&apos;s
              own result manifest records those inputs with its delivery Artifact.
            </span>
          </label>
          <div className="grid gap-2 rounded-md border p-3 md:col-span-2 md:grid-cols-2">
            <label className="space-y-1 text-xs">
              <span>Approval Gate (optional)</span>
              <select
                className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                value={creation.approvalGateId}
                onChange={(event) => setCreation((current) => ({
                  ...current,
                  approvalGateId: event.target.value,
                }))}
              >
                <option value="">No review approval requirement</option>
                {(gatesQuery.data ?? []).map((gate) => (
                  <option key={gate.gate.id} value={gate.gate.id}>{gate.gate.gate_kind} · {gate.gate.id}</option>
                ))}
              </select>
            </label>
            <label className="space-y-1 text-xs">
              <span>Review line</span>
              <select
                className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                value={creation.approvalLineId}
                onChange={(event) => setCreation((current) => ({
                  ...current,
                  approvalLineId: event.target.value,
                }))}
              >
                <option value="">Choose the exact review line</option>
                {(reviewLinesQuery.data ?? []).map((line) => (
                  <option key={line.id} value={line.id}>
                    {workUnits.find((unit) => unit.id === line.current_work_unit_id)?.title ?? line.current_work_unit_id} · {line.latest_verdict ?? 'pending'} · r{String(line.revision)}
                  </option>
                ))}
              </select>
            </label>
            <p className="text-xs text-muted-foreground md:col-span-2">
              The selected Gate&apos;s active review policy must name this exact line. A new correction
              advances the line and makes an older Gate evaluation unusable.
            </p>
          </div>
          <label className="space-y-1 text-xs">
            <span>Parent WorkUnit</span>
            <select
              className="h-9 rounded-md border bg-background px-2 text-sm"
              value={creation.parentId}
              onChange={(event) =>
                setCreation((current) => ({ ...current, parentId: event.target.value }))
              }
            >
              <option value="">No parent</option>
              {workUnits.map((unit) => (
                <option key={unit.id} value={unit.id}>
                  {unit.title}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1 text-xs">
            <span>Initial assignee (optional)</span>
            <select
              className="h-9 rounded-md border bg-background px-2 text-sm"
              value={creation.assignedActor}
              onChange={(event) =>
                setCreation((current) => ({ ...current, assignedActor: event.target.value }))
              }
            >
              <option value="">Unassigned</option>
              {eligibleActors(creation.role || task.task_roles[0]?.role || '').map((option) => (
                <option key={option.key} value={option.key}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <label className="flex items-center gap-2 text-sm md:col-span-2">
            <input
              type="checkbox"
              checked={creation.requiresIntegration}
              onChange={(event) =>
                setCreation((current) => ({
                  ...current,
                  requiresIntegration: event.target.checked,
                }))
              }
            />{' '}
            Use repository Workspace (integration stays explicit)
          </label>
          <div className="grid gap-2 md:col-span-2 md:grid-cols-3">
            <label className="space-y-1 text-xs">
              <span>Provenance kind (optional)</span>
              <select
                className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                value={creation.provenanceKind}
                onChange={(event) =>
                  setCreation((current) => ({
                    ...current,
                    provenanceKind: event.target.value,
                    provenanceRef: '',
                    provenanceActor: '',
                  }))
                }
              >
                <option value="">None</option>
                <option value="actor">Actor</option>
                <option value="work_unit">WorkUnit</option>
                <option value="artifact">Artifact</option>
                <option value="external">External reference</option>
              </select>
            </label>
            {creation.provenanceKind === 'actor' ? (
              <label className="space-y-1 text-xs">
                <span>Provenance Actor</span>
                <select
                  className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                  value={creation.provenanceActor}
                  onChange={(event) =>
                    setCreation((current) => ({ ...current, provenanceActor: event.target.value }))
                  }
                >
                  <option value="">Choose Actor</option>
                  {[
                    ...authorizedAgents.map((agent) => ({
                      value: `agent:${agent.id}`,
                      label: `Agent ${agent.name}`,
                    })),
                    ...projectMembers.map((member) => ({
                      value: `human:${member.user_id}`,
                      label: `Human ${member.display_name || member.email}`,
                    })),
                  ].map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            {creation.provenanceKind && creation.provenanceKind !== 'actor' ? (
              <label className="space-y-1 text-xs">
                <span>Provenance reference</span>
                <Input
                  value={creation.provenanceRef}
                  onChange={(event) =>
                    setCreation((current) => ({ ...current, provenanceRef: event.target.value }))
                  }
                />
              </label>
            ) : null}
          </div>
          <div className="md:col-span-2">
            <Button type="submit" disabled={task.task_roles.length === 0}>
              Create WorkUnit
            </Button>
          </div>
        </form>
      </div>

      <div className="space-y-3 border-t pt-4">
        <h3 className="font-medium">WorkUnits</h3>
        {workUnits.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No WorkUnits have been created for this Task.
          </p>
        ) : null}
        {workUnits.map((unit) => (
          <WorkUnitCard
            key={unit.id}
            unit={unit}
            units={workUnits}
            roles={task.task_roles}
            agents={authorizedAgents}
            members={projectMembers}
            executions={executions}
            draft={
              unitDrafts[unit.id] ?? {
                title: unit.title,
                scope: unit.scope,
                executionInstructions: unit.execution_instructions ?? '',
                requiresIntegration: unit.requires_integration,
              }
            }
            allocation={
              allocations[unit.id] ?? { role: unit.role, actor: actorKey(unit.assigned_actor) }
            }
            dependencyTarget={dependencyTargets[unit.id] ?? ''}
            readiness={freshReadiness[unit.id] ?? unit.readiness}
            onDraftChange={(draft) =>
              setUnitDrafts((current) => ({ ...current, [unit.id]: draft }))
            }
            onAllocationChange={(allocation) =>
              setAllocations((current) => ({ ...current, [unit.id]: allocation }))
            }
            onDependencyTargetChange={(id) =>
              setDependencyTargets((current) => ({ ...current, [unit.id]: id }))
            }
            onSave={() => void saveWorkUnit(unit)}
            onAllocate={() => void allocateWorkUnit(unit)}
            onTransition={(status) => void transitionWorkUnit(unit, status)}
            onAddDependency={() => void createDependency(unit)}
            onRemoveDependency={(id) => void removeDependency(unit, id)}
            onRefreshReadiness={() => void refreshReadiness(unit)}
            onPrepareWorkspace={() => void prepareWorkspace(unit)}
            onIntegrate={(executionId) => void integrateWorkUnit(unit, executionId)}
          />
        ))}
      </div>
    </section>
  )
}

type WorkUnitCardProps = {
  unit: WorkUnitResponse
  units: WorkUnitResponse[]
  roles: TaskRoleResponse[]
  agents: Array<{ id: string; name: string }>
  members: ProjectMemberResponse[]
  executions: Execution[]
  draft: UnitDraft
  allocation: { role: string; actor: string }
  dependencyTarget: string
  readiness: WorkUnitReadinessResponse
  onDraftChange: (draft: UnitDraft) => void
  onAllocationChange: (allocation: { role: string; actor: string }) => void
  onDependencyTargetChange: (id: string) => void
  onSave: () => void
  onAllocate: () => void
  onTransition: (status: WorkUnitStatus) => void
  onAddDependency: () => void
  onRemoveDependency: (id: string) => void
  onRefreshReadiness: () => void
  onPrepareWorkspace: () => void
  onIntegrate: (executionId: string) => void
}

function WorkUnitCard(props: WorkUnitCardProps) {
  const { unit, units, roles, agents, members, executions } = props
  const reviewSubject = unit.review_subject
  const reviewedUnit = units.find((item) => item.id === reviewSubject?.reviewed_work_unit_id)
  const dependenciesQuery = useQuery({
    queryKey: ['work-unit-dependencies', unit.id, unit.version],
    queryFn: () => apiFetch<WorkUnitDependencyResponse[]>(`/work-units/${unit.id}/dependencies`),
  })
  const activeMembers = actorOptions(
    roles.find((role) => role.role === props.allocation.role),
    agents,
    members,
  )
  const completedExecutions = executions.filter((execution) => execution.status === 'completed')
  const readiness = props.readiness
  const isFormalReviewUnit = hasFormalReview(roles.find((role) => role.role === unit.role))
  const artifactPackageQuery = useWorkUnitArtifactPackageQuery(unit.id)
  return (
    <article className="space-y-3 rounded-md border p-3">
      <div className="flex flex-wrap items-center gap-2">
        <strong>{unit.title}</strong>
        <span className="text-xs text-muted-foreground">
          {unit.status} · v{unit.version} · {unit.role}
        </span>
        <code className="ml-auto text-xs text-muted-foreground">{unit.id}</code>
      </div>
      {isFormalReviewUnit ? (
        <div className="rounded bg-amber-50/60 p-2 text-xs" aria-label="Reviewer subject">
          {reviewSubject ? (
            <>
              <strong>Exact review subject:</strong>{' '}
              {reviewedUnit?.title ?? reviewSubject.reviewed_work_unit_id}
              {reviewSubject.reviewed_work_unit_version != null
                ? ` · version ${String(reviewSubject.reviewed_work_unit_version)}`
                : ' · waiting for the exact WorkUnit result'}
              {reviewSubject.result_execution_id
                ? ` · result Execution ${reviewSubject.result_execution_id}`
                : ''}
              {reviewSubject.result_artifact_id
                ? ` · Artifact ${reviewSubject.result_artifact_id}`
                : ''}
              {reviewSubject.result_commit_sha
                ? ` · commit ${reviewSubject.result_commit_sha}`
                : ''}
              <span className="mt-1 block text-muted-foreground">
                This subject is fixed. Create a new reviewer WorkUnit to review another scope.
              </span>
            </>
          ) : (
            <span>No exact subject is attached. Create a new reviewer WorkUnit and select its subject.</span>
          )}
        </div>
      ) : null}
      <div className="grid gap-2 md:grid-cols-2">
        <label className="space-y-1 text-xs">
          <span>Title</span>
          <Input
            value={props.draft.title}
            onChange={(event) => props.onDraftChange({ ...props.draft, title: event.target.value })}
          />
        </label>
        <label className="space-y-1 text-xs">
          <span>WorkUnit status</span>
          <select
            className="h-9 rounded-md border bg-background px-2 text-sm"
            value={unit.status}
            onChange={(event) => props.onTransition(event.target.value as WorkUnitStatus)}
          >
            {WORK_UNIT_STATUSES.map((status) => (
              <option key={status} value={status}>
                {status}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1 text-xs md:col-span-2">
          <span>Scope</span>
          <Textarea
            value={props.draft.scope}
            onChange={(event) => props.onDraftChange({ ...props.draft, scope: event.target.value })}
          />
        </label>
        <label className="space-y-1 text-xs md:col-span-2">
          <span>Harness execution instructions</span>
          <Textarea
            value={props.draft.executionInstructions}
            onChange={(event) =>
              props.onDraftChange({ ...props.draft, executionInstructions: event.target.value })
            }
            placeholder="Optional exact instructions for the assigned harness"
          />
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={props.draft.requiresIntegration}
            onChange={(event) =>
              props.onDraftChange({ ...props.draft, requiresIntegration: event.target.checked })
            }
          />{' '}
            Use repository Workspace (integration stays explicit)
        </label>
        <div>
          <Button size="sm" variant="outline" onClick={props.onSave}>
            Save details
          </Button>
        </div>
      </div>
      <div className="grid gap-2 rounded bg-muted/20 p-3 md:grid-cols-[1fr_1fr_auto]">
        <label className="space-y-1 text-xs">
          <span>Allocated TaskRole</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={props.allocation.role}
            onChange={(event) => props.onAllocationChange({ role: event.target.value, actor: '' })}
          >
            {roles.map((role) => (
              <option
                key={role.id}
                value={role.role}
                disabled={
                  reviewSubject ? !hasFormalReview(role) : hasFormalReview(role)
                }
              >
                {role.role}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1 text-xs">
          <span>Allocated Actor</span>
          <select
            className="h-9 w-full rounded-md border bg-background px-2 text-sm"
            value={props.allocation.actor}
            onChange={(event) =>
              props.onAllocationChange({ ...props.allocation, actor: event.target.value })
            }
          >
            <option value="">Unassigned</option>
            {activeMembers.map((option) => (
              <option key={option.key} value={option.key}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <div className="flex items-end">
          <Button size="sm" variant="outline" onClick={props.onAllocate}>
            Save allocation
          </Button>
        </div>
      </div>
      <div className="space-y-2 rounded bg-muted/20 p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h4 className="text-sm font-medium">Dependencies</h4>
          <div className="flex gap-2">
            {unit.requires_integration ? (
              <Button size="sm" variant="outline" onClick={props.onPrepareWorkspace}>
                Prepare workspace
              </Button>
            ) : null}
            <Button size="sm" variant="ghost" onClick={props.onRefreshReadiness}>
              Refresh readiness
            </Button>
          </div>
        </div>
        <p className="text-xs text-muted-foreground">
          Runnable: {String(readiness.runnable)} · ready for allocation:{' '}
          {String(readiness.ready_for_allocation)} · awaiting integration:{' '}
          {String(readiness.awaiting_integration)} · awaiting acceptance:{' '}
          {String(readiness.awaiting_acceptance)} · awaiting review approval:{' '}
          {String(readiness.awaiting_review_approval)}
          {readiness.active_execution_ids.length
            ? ` · active Executions: ${readiness.active_execution_ids.join(', ')}`
            : ''}
          {readiness.unsatisfied_dependency_ids.length
            ? ` · unsatisfied: ${readiness.unsatisfied_dependency_ids.join(', ')}`
            : ''}
        </p>
        {dependenciesQuery.isError ? (
          <p role="alert" className="text-xs text-destructive">
            {getApiErrorMessage(dependenciesQuery.error, 'Could not load dependencies')}{' '}
            <button
              type="button"
              className="underline"
              onClick={() => void dependenciesQuery.refetch()}
            >
              Retry
            </button>
          </p>
        ) : null}
        {dependenciesQuery.data?.map((dependency) => (
          <div key={dependency.depends_on_work_unit_id} className="flex items-center gap-2 text-xs">
            <span className="flex-1">
              Depends on{' '}
              {units.find((item) => item.id === dependency.depends_on_work_unit_id)?.title ??
                dependency.depends_on_work_unit_id}{' '}
              · {dependency.satisfied ? 'satisfied' : 'open'}
            </span>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => props.onRemoveDependency(dependency.depends_on_work_unit_id)}
            >
              Remove
            </Button>
          </div>
        ))}
        <div className="flex gap-2">
          <select
            aria-label={`Add dependency to ${unit.title}`}
            className="h-9 min-w-0 flex-1 rounded-md border bg-background px-2 text-sm"
            value={props.dependencyTarget}
            onChange={(event) => props.onDependencyTargetChange(event.target.value)}
          >
            <option value="">Choose prerequisite WorkUnit</option>
            {units
              .filter(
                (candidate) =>
                  candidate.id !== unit.id &&
                  !dependenciesQuery.data?.some(
                    (dependency) => dependency.depends_on_work_unit_id === candidate.id,
                  ),
              )
              .map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.title}
                </option>
              ))}
          </select>
          <Button
            size="sm"
            variant="outline"
            disabled={!props.dependencyTarget}
            onClick={props.onAddDependency}
          >
            Add dependency
          </Button>
        </div>
      </div>
      {unit.requires_integration ? (
        <div className="flex flex-wrap items-end gap-2 rounded bg-muted/20 p-3">
          <label className="min-w-56 flex-1 space-y-1 text-xs">
            <span>Completed Execution to integrate</span>
            <select
              key={unit.version}
              aria-label={`Execution to integrate ${unit.title}`}
              className="h-9 w-full rounded-md border bg-background px-2 text-sm"
              defaultValue=""
              onChange={(event) => {
                if (event.target.value) props.onIntegrate(event.target.value)
              }}
            >
              <option value="">Select exact completed Execution</option>
              {completedExecutions.map((execution) => (
                <option key={execution.id} value={execution.id}>
                  {execution.role} · {execution.actor_ref?.kind}:
                  {execution.actor_ref?.id} · {execution.id}
                </option>
              ))}
            </select>
          </label>
          <p className="text-xs text-muted-foreground">
            The API validates that the selected Execution produced this WorkUnit&apos;s current
            result.
          </p>
        </div>
      ) : null}
      {artifactPackageQuery.data &&
      (artifactPackageQuery.data.inputs.length > 0 || artifactPackageQuery.data.manifests.length > 0) ? (
        <section className="space-y-2 rounded-md border p-3 text-xs" aria-label="WorkUnit Artifact package">
          <h4 className="font-medium">Exact Artifact bindings and result manifests</h4>
          {artifactPackageQuery.data.inputs.length ? (
            <div className="space-y-1">
              <strong>Inputs</strong>
              {artifactPackageQuery.data.inputs.map((input) => (
                <p key={input.artifact_id} className="break-all">
                  Artifact {input.artifact_id} · digest {input.digest} · producer {input.producer_execution_id}
                </p>
              ))}
            </div>
          ) : null}
          {artifactPackageQuery.data.manifests.map((manifest) => (
            <details key={manifest.id} className="rounded bg-muted/30 p-2">
              <summary className="cursor-pointer break-all">
                Result {manifest.result_execution_id} · delivery {manifest.delivery_artifact_id} · manifest {manifest.manifest_digest}
              </summary>
              <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-all">{JSON.stringify(manifest.manifest, null, 2)}</pre>
            </details>
          ))}
        </section>
      ) : null}
      {artifactPackageQuery.isError ? (
        <p role="alert" className="text-xs text-destructive">
          Artifact bindings could not be loaded.{' '}
          <button type="button" className="underline" onClick={() => void artifactPackageQuery.refetch()}>
            Retry
          </button>
        </p>
      ) : null}
    </article>
  )
}
