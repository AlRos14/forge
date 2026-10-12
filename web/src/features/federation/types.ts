import type { AgentStatus } from '@/types/generated'

export type JsonObject = Record<string, unknown>

export interface Page<T> {
  items: T[]
  next_cursor?: string | null
  has_more: boolean
  total_count?: number | null
}

export interface FederatedAgent {
  id: string
  name: string
  description: string | null
  profile_id: string
  executor_type: string
  provider: string | null
  model: string | null
  reasoning_effort: string | null
  permission_policy: string | null
  prompt_template: string | null
  capabilities: string[]
  config_json: JsonObject
  credential_handle_id: string | null
  daemon_id: string | null
  max_concurrent_tasks: number
  status: AgentStatus
  active_execution_count: number | null
  effective_status: string | null
  total_runs: number
  avg_duration_ms: number | null
  success_rate: number | null
  is_default: boolean
  paused: boolean
  owner_id: string | null
  visibility: string
  version: number
  created_at: string
  updated_at: string
}

export interface AgentUsage {
  available: boolean
  executor_type: string
  account_key: string | null
  daemon_id: string | null
  shared_account: boolean
  source: string | null
  usage: JsonObject | null
  captured_at: string | null
  stale: boolean
  message: string | null
}

export interface AgentProfile {
  id: string
  identity_id: string
  executor_type: string
  provider: string | null
  model: string | null
  reasoning_effort: string | null
  permission_policy: string | null
  system_prompt: string | null
  capabilities: JsonObject
  tool_policy: JsonObject
  config: JsonObject
  credential_handle_id: string | null
  version: number
  created_at: string
}

export interface ProviderUsageWindow {
  id: 'primary' | 'secondary' | string
  used_percent: number
  window_minutes: number | null
  resets_at: string | null
}

export interface ProviderUsage {
  id: string
  provider: string
  source: 'probe' | 'unknown'
  plan_type?: string | null
  windows: ProviderUsageWindow[]
  fetched_at: string
  detail?: string | null
}
