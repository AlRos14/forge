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
