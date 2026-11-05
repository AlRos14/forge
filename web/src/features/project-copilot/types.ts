import type { ProjectCopilotRunStatus } from '@/types/generated'

export type {
  ProjectCopilotActivity,
  ProjectCopilotActivityResponse,
  ProjectCopilotMessage,
  ProjectCopilotMessagesResponse,
  ProjectCopilotResponse,
  ProjectCopilotRun,
  ProjectCopilotRunDetailResponse,
  ProjectCopilotRunStatus,
  SendProjectCopilotMessageRequest,
  SendProjectCopilotMessageResponse,
  UpdateProjectCopilotRequest,
} from '@/types/generated'

export function isProjectCopilotRunPending(status: ProjectCopilotRunStatus | undefined) {
  return status === 'queued' || status === 'running'
}

export function isProjectCopilotRunTerminal(status: ProjectCopilotRunStatus | undefined) {
  return status !== undefined && !isProjectCopilotRunPending(status)
}
