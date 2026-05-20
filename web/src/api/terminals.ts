import { apiFetch } from '@/api/client'
import type {
  CreateTerminalSessionRequest,
  ResizeTerminalSessionRequest,
  TerminalAttachTokenResponse,
  TerminalAvailability,
  TerminalSessionResponse,
} from '@/types/generated'

export type CreateTerminalSessionResult = {
  session: TerminalSessionResponse
  attach: TerminalAttachTokenResponse
}

export type ListTerminalSessionsOptions = {
  includeEnded?: boolean
}

export function createTerminalSession(
  taskId: string,
  body: CreateTerminalSessionRequest,
): Promise<CreateTerminalSessionResult> {
  return apiFetch<CreateTerminalSessionResult>(`/tasks/${taskId}/terminals`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function listTerminalSessions(
  taskId: string,
  opts?: ListTerminalSessionsOptions,
): Promise<TerminalSessionResponse[]> {
  return apiFetch<TerminalSessionResponse[]>(`/tasks/${taskId}/terminals`, {
    search: { include_ended: opts?.includeEnded },
  })
}

export function getTerminalSession(sessionId: string): Promise<TerminalSessionResponse> {
  return apiFetch<TerminalSessionResponse>(`/terminals/${sessionId}`)
}

export function getTerminalAvailability(taskId: string): Promise<TerminalAvailability> {
  return apiFetch<TerminalAvailability>(`/tasks/${taskId}/terminals/availability`)
}

export function issueTerminalAttachToken(
  sessionId: string,
): Promise<TerminalAttachTokenResponse> {
  return apiFetch<TerminalAttachTokenResponse>(`/terminals/${sessionId}/attach-token`, {
    method: 'POST',
  })
}

export function resizeTerminalSession(
  sessionId: string,
  body: ResizeTerminalSessionRequest,
): Promise<TerminalSessionResponse> {
  return apiFetch<TerminalSessionResponse>(`/terminals/${sessionId}/resize`, {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function terminateTerminalSession(
  sessionId: string,
  reason?: string,
): Promise<TerminalSessionResponse> {
  return apiFetch<TerminalSessionResponse>(`/terminals/${sessionId}/terminate`, {
    method: 'POST',
    body: reason === undefined ? undefined : JSON.stringify({ reason }),
  })
}

export function terminalWebSocketUrl(sessionId: string, attachToken: string): string {
  const url = new URL(
    `/api/v1/terminals/${encodeURIComponent(sessionId)}/ws`,
    window.location.origin,
  )
  url.protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('attach_token', attachToken)
  return url.toString()
}
