import type { Execution } from '@/types/generated'

export function isResumeExecution(execution: Execution): boolean {
  const snapshot = execution.executor_config_snapshot
  if (!snapshot) return false
  const dispatch = snapshot.dispatch as Record<string, unknown> | undefined
  if (dispatch?.execution_policy === 'resume_latest_target_role_thread') return true

  // Check dispatch metadata set by executor_snapshot_with_resume_thread.
  const meta = snapshot.dispatch_metadata as Record<string, unknown> | undefined
  if (meta?.execution_policy === 'resume_latest_target_role_thread') return true

  // Fallback: check executor-specific resume config fields
  const config = snapshot.config as Record<string, unknown> | undefined
  return config?.resume_thread_in_place === true || typeof config?.resume_session_id === 'string'
}

export function roleDisplayName(role: string): string {
  const names: Record<string, string> = {
    executor: 'Executor',
    coder: 'Coder',
    planner: 'Planner',
    reviewer: 'Reviewer',
    auditor: 'Auditor',
    merge_fixer: 'Merge Fixer',
    interactive: 'Interactive',
  }
  return names[role] ?? role.charAt(0).toUpperCase() + role.slice(1).replace(/_/g, ' ')
}

export function turnLabel(index: number, execution: Execution): string {
  if (index === 0) return 'Initial run'
  if (isResumeExecution(execution)) return 'Follow-up turn'
  return 'Re-execution'
}

export type ExecutionChain = { root: Execution; turns: Execution[] }

export function buildExecutionChains(executions: Execution[]): ExecutionChain[] {
  const byId = new Map(executions.map((e) => [e.id, e]))
  const childrenOf = new Map<string, Execution[]>()

  for (const e of executions) {
    if (e.parent_execution_id && byId.has(e.parent_execution_id)) {
      const arr = childrenOf.get(e.parent_execution_id) ?? []
      arr.push(e)
      childrenOf.set(e.parent_execution_id, arr)
    }
  }

  // Roots: no parent, or parent not in this list
  const roots = executions.filter(
    (e) => !e.parent_execution_id || !byId.has(e.parent_execution_id),
  )
  roots.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())

  const chains: ExecutionChain[] = []
  for (const root of roots) {
    const turns: Execution[] = []
    const queue: Execution[] = [root]
    const seen = new Set<string>()
    while (queue.length > 0) {
      const curr = queue.shift()!
      if (seen.has(curr.id)) continue
      seen.add(curr.id)
      turns.push(curr)
      const children = (childrenOf.get(curr.id) ?? []).sort(
        (a, b) => new Date(a.created_at).getTime() - new Date(b.created_at).getTime(),
      )
      queue.push(...children)
    }
    chains.push({ root, turns })
  }

  return chains.sort((a, b) => {
    const aLatest = a.turns[a.turns.length - 1]
    const bLatest = b.turns[b.turns.length - 1]
    if (aLatest.status === 'running' && bLatest.status !== 'running') return -1
    if (bLatest.status === 'running' && aLatest.status !== 'running') return 1
    return new Date(bLatest.created_at).getTime() - new Date(aLatest.created_at).getTime()
  })
}
