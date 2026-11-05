import { apiFetch } from '@/api/client'
import type {
  ProjectCopilotActivityResponse,
  ProjectCopilotMessagesResponse,
  ProjectCopilotResponse,
  ProjectCopilotRunDetailResponse,
  ProjectCopilotRun,
  SendProjectCopilotMessageRequest,
  SendProjectCopilotMessageResponse,
  UpdateProjectCopilotRequest,
} from '@/features/project-copilot/types'

function projectCopilotPath(projectId: string) {
  return `/projects/${encodeURIComponent(projectId)}/copilot`
}

export function getProjectCopilot(projectId: string) {
  return apiFetch<ProjectCopilotResponse>(projectCopilotPath(projectId))
}

export function updateProjectCopilot(projectId: string, body: UpdateProjectCopilotRequest) {
  return apiFetch<ProjectCopilotResponse>(projectCopilotPath(projectId), {
    method: 'PUT',
    body: JSON.stringify(body),
  })
}

export function listProjectCopilotMessages(
  projectId: string,
  cursor: string | undefined,
  limit: number,
) {
  return apiFetch<ProjectCopilotMessagesResponse>(`${projectCopilotPath(projectId)}/messages`, {
    search: { cursor, limit },
  })
}

export function listProjectCopilotActivity(
  projectId: string,
  cursor: string | undefined,
  limit: number,
) {
  return apiFetch<ProjectCopilotActivityResponse>(`${projectCopilotPath(projectId)}/activity`, {
    search: { cursor, limit },
  })
}

export function sendProjectCopilotMessage(
  projectId: string,
  body: SendProjectCopilotMessageRequest,
) {
  return apiFetch<SendProjectCopilotMessageResponse>(`${projectCopilotPath(projectId)}/messages`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function getProjectCopilotRun(projectId: string, runId: string) {
  return apiFetch<ProjectCopilotRunDetailResponse>(
    `${projectCopilotPath(projectId)}/runs/${encodeURIComponent(runId)}`,
  )
}

export function cancelProjectCopilotRun(projectId: string, runId: string) {
  return apiFetch<ProjectCopilotRun>(
    `${projectCopilotPath(projectId)}/runs/${encodeURIComponent(runId)}`,
    { method: 'DELETE' },
  )
}
