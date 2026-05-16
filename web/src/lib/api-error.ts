import { toast } from 'sonner'
import { ApiError } from '@/api/client'

export function getApiErrorMessage(error: unknown, fallback = 'Request failed'): string {
  if (error instanceof ApiError) {
    let message = error.message || fallback
    let requestId = error.requestId
    try {
      const parsed = JSON.parse(error.message) as {
        message?: unknown
        request_id?: unknown
      }
      if (typeof parsed.message === 'string' && parsed.message) {
        message = parsed.message
      }
      if (typeof parsed.request_id === 'string' && parsed.request_id) {
        requestId = parsed.request_id
      }
    } catch {
      // Non-JSON error bodies are already usable as-is.
    }
    return `${message}${requestId ? ` Request ID: ${requestId}` : ''}`
  }
  if (error instanceof Error) return error.message
  return fallback
}

export function toastApiError(error: unknown, fallback?: string): void {
  toast.error(getApiErrorMessage(error, fallback))
}

export function isApiStatus(error: unknown, status: number): boolean {
  return error instanceof ApiError && error.status === status
}
