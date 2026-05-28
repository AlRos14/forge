import type { Agent, AgentStatus, Daemon } from '@/types/generated'

export const executorTypes = ['claude_code', 'codex', 'cursor', 'gemini', 'opencode', 'shell', 'null'] as const

export const executorDisplayNames: Record<string, string> = {
  claude_code: 'Claude Code',
  codex: 'Codex',
  cursor: 'Cursor',
  gemini: 'Gemini',
  opencode: 'OpenCode',
  shell: 'Shell',
  null: 'Null (no-op)',
}

export const EMPTY_AGENTS: Agent[] = []
export const EMPTY_DAEMONS: Daemon[] = []
export const AGENTS_PAGE_SIZE = 20

export const statusConfig: Record<AgentStatus, { dot: string; label: string }> = {
  idle: { dot: 'bg-orange-500', label: 'Idle' },
  busy: { dot: 'bg-amber-400', label: 'Busy' },
  error: { dot: 'bg-red-500', label: 'Error' },
  offline: { dot: 'bg-stone-400', label: 'Offline' },
}

export const effectiveStatusLabels: Record<string, string> = {
  error: 'Error',
  daemon_offline: 'Daemon offline',
  deactivated: 'CLI not authenticated',
}
