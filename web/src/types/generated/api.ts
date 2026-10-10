// Handwritten names for UI conveniences around Rust-generated wire DTOs.
// Public response fields themselves come from the api-types bindings.

export type { AgentResponse as Agent } from './bindings/AgentResponse'
export type { AgentAvailabilityResponse as AgentAvailability } from './bindings/AgentAvailabilityResponse'
export type { DiscoveredOptionsResponse as AgentDiscoveredOptions } from './bindings/DiscoveredOptionsResponse'
export type { DaemonResponse as Daemon } from './bindings/DaemonResponse'
export type { ExecutionResponse as Execution } from './bindings/ExecutionResponse'
export type { ExecutionUsageResponse as ExecutionUsage } from './bindings/ExecutionUsageResponse'
export type { ExecutorTypeDescriptor as ExecutorType } from './bindings/ExecutorTypeDescriptor'
export type { ExecutionLogEntry as LogEntry } from './bindings/ExecutionLogEntry'
export type { ProjectResponse as Project } from './bindings/ProjectResponse'
export type { RepoResponse as Repo } from './bindings/RepoResponse'
export type { TaskResponse as Task } from './bindings/TaskResponse'
export type { TaskUsageSummaryResponse as TaskUsageSummary } from './bindings/TaskUsageSummaryResponse'
export type { WorkspaceResponse as Workspace } from './bindings/WorkspaceResponse'
export type { PublicEventEnvelope as ForgeEvent } from './bindings/PublicEventEnvelope'

export type PaginatedResponse<T> = {
  items: T[]
  next_cursor: string | null
  has_more: boolean
  total_count: number | null
}
