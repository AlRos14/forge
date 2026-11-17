import { apiFetch } from '@/api/client'
import type {
  ApplyProjectPlanRequest,
  AuthorizeProjectPlanBatchRequest,
  ProjectPlanBatchResponse,
  ProjectPlanGraphResponse,
  UpdateProjectPlanBatchRequest,
} from '@/types/generated'

const planPath = (projectId: string) => `/projects/${encodeURIComponent(projectId)}/plan`

export function getProjectPlan(projectId: string) {
  return apiFetch<ProjectPlanGraphResponse>(planPath(projectId))
}

export function applyProjectPlan(
  projectId: string,
  proposalId: string,
  body: ApplyProjectPlanRequest,
) {
  return apiFetch<ProjectPlanGraphResponse>(
    `${planPath(projectId)}/proposals/${encodeURIComponent(proposalId)}/apply`,
    {
      method: 'POST',
      body: JSON.stringify(body),
    },
  )
}

export function authorizeProjectPlanBatch(
  projectId: string,
  body: AuthorizeProjectPlanBatchRequest,
) {
  return apiFetch<ProjectPlanBatchResponse>(`${planPath(projectId)}/batches`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function updateProjectPlanBatch(
  projectId: string,
  batchId: string,
  body: UpdateProjectPlanBatchRequest,
) {
  return apiFetch<ProjectPlanBatchResponse>(
    `${planPath(projectId)}/batches/${encodeURIComponent(batchId)}`,
    {
      method: 'POST',
      body: JSON.stringify(body),
    },
  )
}
