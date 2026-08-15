import { apiFetch } from '@/api/client'
import type {
  AuthorizationProvenance,
  CreateDecisionCandidateRequest,
  CreateMilestoneRequest,
  CreateProjectDocumentRequest,
  DecisionCandidate,
  MilestoneDefinitionContent,
  MutationEnvelope,
  ProjectDocument,
  ProjectMilestone,
  RevisionProvenance,
  UserResponse,
} from '@/types/generated'

type JsonMutationEnvelope = Omit<MutationEnvelope, 'expected_version'> & {
  expected_version: number
}

type JsonCreateDocumentRequest = Omit<CreateProjectDocumentRequest, 'mutation'> & {
  mutation: JsonMutationEnvelope
}
type JsonCreateDecisionRequest = Omit<CreateDecisionCandidateRequest, 'mutation'> & {
  mutation: JsonMutationEnvelope
}
type JsonCreateMilestoneRequest = Omit<CreateMilestoneRequest, 'mutation'> & {
  mutation: JsonMutationEnvelope
}

function requestId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`
}

function authorization(user: UserResponse, action: string): AuthorizationProvenance {
  return {
    principal: {
      kind: 'user',
      id: user.id,
      display_name: user.display_name ?? user.email,
    },
    authorization_basis: 'interactive_user_action',
    action,
    event_id: requestId(),
    occurred_at: new Date().toISOString(),
  }
}

function mutation(user: UserResponse, action: string, version: number): JsonMutationEnvelope {
  return {
    expected_version: version,
    expected_digest: null,
    idempotency_key: requestId(),
    deduplication_key: null,
    authorization: authorization(user, action),
  }
}

function canonicalValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalValue)
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, entry]) => [key, canonicalValue(entry)]),
    )
  }
  return value
}

export function canonicalJson(value: unknown): string {
  return JSON.stringify(canonicalValue(value))
}

export function createWorkbenchDocument(
  projectId: string,
  user: UserResponse,
  projectVersion: number,
  input: Pick<CreateProjectDocumentRequest, 'kind' | 'title'>,
): Promise<Pick<ProjectDocument, 'id' | 'title'>> {
  const body: JsonCreateDocumentRequest = {
    mutation: mutation(user, 'project.document.create', projectVersion),
    kind: input.kind,
    title: input.title,
    approval_policy: 'user_or_project_agent',
  }
  return apiFetch(`/projects/${projectId}/documents`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function createWorkbenchDecision(
  projectId: string,
  user: UserResponse,
  projectVersion: number,
  input: { question: string; options: string[]; rationale: string | null },
): Promise<Pick<DecisionCandidate, 'id' | 'question'>> {
  const body: JsonCreateDecisionRequest = {
    mutation: mutation(user, 'project.decision.candidate.create', projectVersion),
    question: input.question,
    context: {
      summary: input.rationale,
      constraints: [],
      affected_artifact_refs: [],
      affected_task_ids: [],
      affected_milestone_ids: [],
      governing_charter_revision_id: null,
      governing_baseline_revision_id: null,
      supersedes_decision_id: null,
      invalidates_decision_id: null,
    },
    options: input.options,
    selected_outcome: null,
    rationale: input.rationale,
    decision_class: 'project_implementation',
    source_refs: [],
  }
  return apiFetch(`/projects/${projectId}/decisions/candidates`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function createWorkbenchMilestone(
  projectId: string,
  user: UserResponse,
  projectVersion: number,
  input: { name: string; outcome: string },
): Promise<Pick<ProjectMilestone, 'id' | 'canonical_id'>> {
  const content: MilestoneDefinitionContent = {
    name: input.name,
    outcome: input.outcome,
    included_scope: [],
    excluded_scope: [],
    charter_revision: null,
    document_revisions: [],
    task_ids: [],
    dependencies: [],
    risks: [],
    acceptance_checks: [],
    evidence_requirements: [],
    known_issues: [],
    target_date: null,
  }
  const provenance: RevisionProvenance = {
    author: {
      kind: 'user',
      id: user.id,
      display_name: user.display_name ?? user.email,
    },
    profile_revision: null,
    operating_skill_revision: null,
    source_refs: [],
    change_summary: 'Created from the Project Agent Workspace',
    material_diff: null,
  }
  const body: JsonCreateMilestoneRequest = {
    mutation: mutation(user, 'project.milestone.create', projectVersion),
    display_label: input.name,
    lifecycle: 'draft',
    content,
    rendered_view: canonicalJson(content),
    render_version: 'forge.milestone-definition-render/v1',
    change_summary: 'Created from the Project Agent Workspace',
    provenance,
  }
  return apiFetch(`/projects/${projectId}/milestones`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}
