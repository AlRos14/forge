const recentTransitions = new Map<string, number>()
const WINDOW_MS = 4000

export function recordUserInitiatedTransition(taskId: string): void {
  if (!taskId) return
  recentTransitions.set(taskId, Date.now())
}

export function shouldSuppressToast(taskId?: string | null): boolean {
  if (!taskId) return false
  const at = recentTransitions.get(taskId)
  if (!at) return false
  const now = Date.now()
  if (now - at <= WINDOW_MS) {
    recentTransitions.delete(taskId)
    return true
  }
  recentTransitions.delete(taskId)
  return false
}
