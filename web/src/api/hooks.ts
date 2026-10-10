import {
  type InfiniteData,
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query'
import {
  ApiError,
  apiFetch,
  addDependency,
  addMember,
  searchUsers,
  createPat,
  deleteNotification,
  deletePat,
  getOperationsStatus,
  getSettings,
  getUnreadNotificationCount,
  listDependencies,
  listDependents,
  listMembers,
  listNotifications,
  listPats,
  listProjectHookRuns,
  markAllNotificationsRead,
  markNotificationRead,
  refreshOperations,
  removeDependency,
  removeMember,
  updateMe,
  updateMemberRole,
  updateSettings,
} from '@/api/client'
import { qk } from '@/api/query-keys'
import { useAuthStore } from '@/stores/auth'
import type {
  Agent,
  AgentAvailability,
  AgentDiscoveredOptions,
  DiffEnvelope,
  CreateAgentRequest,
  CreateProjectRequest,
  CreateRepoRequest,
  RepoSyncResponse,
  UpdateRepoRequest,
  CreateTaskRequest,
  Daemon,
  Execution,
  ExecutionLogsResponse,
  ExecutionUsage,
  ExecutorType,
  FollowUpRequest,
  StartExecutionRequest,
  McpConfigActionRequest,
  McpConfigResponse,
  OAuthApproveRequest,
  OAuthApproveResponse,
  OAuthAuthorizeContext,
  PaginatedResponse,
  Project,
  ProjectRelease,
  Repo,
  CreateIntegrationRequest,
  PatchIntegrationRequest,
  IntegrationResponse,
  ReviewExecutionResponse,
  StartReviewExecutionRequest,
  SubmitReviewReportRequest,
  SubmitReviewReportResponse,
  ValidationRunResponse,
  Task,
  TaskLifecycleState,
  TaskLifecycleTransitionFactResponse,
  TransitionTaskLifecycleRequest,
  TaskLifecycleTransitionResponse,
  GateResponse,
  GateEvaluationResponse,
  UpdateAgentRequest,
  UpdateProjectRequest,
  UpdateTaskRequest,
  TaskUsageSummary,
  SettingsResponse,
  UpdateSettingsRequest,
  Workspace,
} from '@/types/generated'
import { recordUserInitiatedTransition } from '@/lib/notification-toast-suppression'

type TaskSearch = {
  cursor?: string
  limit?: number
  q?: string
  sort_by?: string
  sort_order?: 'asc' | 'desc'
  lifecycle_state?: TaskLifecycleState | string
}

export type ExecutionLogsParams = {
  tail?: number
  from_sequence?: number
  limit?: number
}

export type CliProjectionItem = {
  daemon_id: string
  daemon_hostname: string
  daemon_status: string
  kind: string
  availability: string
  config_path: string | null
  version: string | null
  path: string | null
  agents: Array<{
    id: string
    name: string
    executor_type: string
    effective_status: string | null
  }>
}

export type CliProjectionResponse = {
  items: CliProjectionItem[]
}

function filterKey(input: TaskSearch): string {
  return JSON.stringify(input)
}

function searchParamsString(query: Partial<Record<string, string>>): string {
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== '') {
      params.set(key, value)
    }
  }
  return params.toString()
}

// --- OAuth ---

export function useOAuthAuthorizeContext(query: Partial<Record<string, string>>) {
  const queryString = searchParamsString(query)
  return useQuery({
    queryKey: ['oauth-authorize-context', JSON.stringify(query)],
    queryFn: () =>
      apiFetch<OAuthAuthorizeContext>(
        `/oauth/authorize/context${queryString ? `?${queryString}` : ''}`,
      ),
    enabled: Boolean(query.client_id && query.redirect_uri && query.response_type),
  })
}

export function useOAuthApprove() {
  return useMutation({
    mutationFn: (input: OAuthApproveRequest) =>
      apiFetch<OAuthApproveResponse>('/oauth/authorize/approve', {
        method: 'POST',
        body: JSON.stringify(input),
      }),
  })
}

// --- Projects ---

export function useProjectsQuery() {
  return useQuery({
    queryKey: qk.projects,
    queryFn: () => apiFetch<PaginatedResponse<Project>>('/projects'),
  })
}

export function useProjectsInfiniteQuery(limit = 20) {
  return useInfiniteQuery({
    queryKey: qk.projectPages(limit),
    queryFn: ({ pageParam }) =>
      apiFetch<PaginatedResponse<Project>>('/projects', {
        search: { cursor: pageParam as string | undefined, limit, include_total: true },
      }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  })
}

export function useProjectQuery(projectId: string) {
  return useQuery({
    queryKey: qk.project(projectId),
    queryFn: () => apiFetch<Project>(`/projects/${projectId}`),
  })
}

export function useProjectReleaseQuery(projectId: string, releaseId: string) {
  return useQuery({
    queryKey: qk.projectRelease(projectId, releaseId),
    queryFn: () => apiFetch<ProjectRelease>(`/projects/${projectId}/releases/${releaseId}`),
    enabled: Boolean(projectId && releaseId),
  })
}

export function useProjectHookRunsQuery(projectId: string, limit = 20) {
  return useInfiniteQuery({
    queryKey: qk.projectHookRunPages(projectId, limit),
    queryFn: ({ pageParam }) => listProjectHookRuns(projectId, pageParam, limit),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  })
}

export function useCreateProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateProjectRequest) =>
      apiFetch<Project>('/projects', {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })
}

export function useUpdateProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ projectId, body }: { projectId: string; body: UpdateProjectRequest }) =>
      apiFetch<Project>(`/projects/${projectId}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    onSuccess: (project) => {
      void queryClient.invalidateQueries({ queryKey: qk.project(project.id) })
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })
}

export function usePauseProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectId: string) =>
      apiFetch<Project>(`/projects/${projectId}/pause`, {
        method: 'POST',
      }),
    onSuccess: (project) => {
      void queryClient.invalidateQueries({ queryKey: qk.project(project.id) })
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })
}

export function useResumeProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectId: string) =>
      apiFetch<Project>(`/projects/${projectId}/resume`, {
        method: 'POST',
      }),
    onSuccess: (project) => {
      void queryClient.invalidateQueries({ queryKey: qk.project(project.id) })
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })
}

export function useDeleteProject() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectId: string) =>
      apiFetch<void>(`/projects/${projectId}`, { method: 'DELETE' }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })
}

// --- Tasks ---

export function useTasksQuery(projectId: string, search: TaskSearch) {
  return useInfiniteQuery({
    queryKey: qk.tasks(projectId, filterKey(search)),
    queryFn: ({ pageParam }) =>
      apiFetch<PaginatedResponse<Task>>(`/projects/${projectId}/tasks`, {
        search: { ...search, cursor: pageParam as string | undefined },
      }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  })
}

export function useTaskQuery(taskId: string) {
  return useQuery({
    queryKey: qk.task(taskId),
    queryFn: () => apiFetch<Task>(`/tasks/${taskId}`),
  })
}

export function useCreateTask(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateTaskRequest) =>
      apiFetch<Task>(`/projects/${projectId}/tasks`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: (task) => {
      void queryClient.invalidateQueries({ queryKey: qk.projectTasks(projectId) })
      void queryClient.invalidateQueries({ queryKey: qk.task(task.id) })
    },
  })
}

export function useUpdateTask() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ taskId, body }: { taskId: string; body: UpdateTaskRequest }) =>
      apiFetch<Task>(`/tasks/${taskId}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    onSuccess: (task) => {
      void queryClient.invalidateQueries({ queryKey: qk.task(task.id) })
      void queryClient.invalidateQueries({ queryKey: qk.projectTasks(task.project_id) })
    },
  })
}

export function useTransitionTaskLifecycle() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      taskId,
      toState,
      expectedLifecycleVersion,
      gateEvaluationId,
      reasonKind,
      reasonRef,
      idempotencyKey,
    }: {
      taskId: string
      toState: TaskLifecycleState
      expectedLifecycleVersion: number
      gateEvaluationId?: string | null
      reasonKind?: string | null
      reasonRef?: string | null
      idempotencyKey: string
    }) => apiFetch<TaskLifecycleTransitionResponse>(`/tasks/${taskId}/lifecycle`, {
      method: 'POST',
      body: JSON.stringify({
        to_state: toState,
        expected_lifecycle_version: expectedLifecycleVersion,
        gate_evaluation_id: gateEvaluationId ?? null,
        reason_kind: reasonKind ?? null,
        reason_ref: reasonRef ?? null,
        idempotency_key: idempotencyKey,
      } satisfies TransitionTaskLifecycleRequest),
    }),
    onMutate: ({ taskId }) => {
      recordUserInitiatedTransition(taskId)
    },
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: qk.task(result.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.taskDetail(result.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.lifecycleTransitions(result.task_id) })
      void queryClient.invalidateQueries({
        predicate: (query) => query.queryKey[0] === 'projects' && query.queryKey[2] === 'tasks',
      })
    },
  })
}

export function useTaskLifecycleTransitionsQuery(taskId: string) {
  return useQuery({
    queryKey: qk.lifecycleTransitions(taskId),
    queryFn: () => apiFetch<TaskLifecycleTransitionFactResponse[]>(`/tasks/${taskId}/lifecycle/transitions`),
    enabled: Boolean(taskId),
  })
}

export function useTaskGatesQuery(taskId: string) {
  return useQuery({
    queryKey: qk.gates(taskId),
    queryFn: () => apiFetch<GateResponse[]>(`/tasks/${taskId}/gates`),
    enabled: Boolean(taskId),
  })
}

export function useEvaluateGate() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (gateId: string) =>
      apiFetch<GateEvaluationResponse>(`/gates/${gateId}/evaluate`, { method: 'POST' }),
    onSuccess: (evaluation) => {
      void queryClient.invalidateQueries({ queryKey: qk.gates(evaluation.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.task(evaluation.task_id) })
    },
  })
}

export function useTriggerReview() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ taskId, body }: { taskId: string; body: StartReviewExecutionRequest }) =>
      apiFetch<ReviewExecutionResponse>(`/tasks/${taskId}/review-executions`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: qk.task(result.execution.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.reviewExecutions(result.execution.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.executions(result.execution.task_id) })
    },
  })
}

export function useReviewsQuery(taskId: string) {
  return useQuery({
    queryKey: qk.reviewExecutions(taskId),
    queryFn: () => apiFetch<ReviewExecutionResponse[]>(`/tasks/${taskId}/review-executions`),
    enabled: Boolean(taskId),
  })
}

export function useSubmitReviewReport() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ executionId, body }: { executionId: string; body: SubmitReviewReportRequest }) =>
      apiFetch<SubmitReviewReportResponse>(`/review-executions/${executionId}`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: (result) => {
      const taskId = result.review_execution.execution.task_id
      void queryClient.invalidateQueries({ queryKey: qk.reviewExecutions(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.executions(taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.task(taskId) })
    },
  })
}

export function useValidationsQuery(taskId: string) {
  return useQuery({
    queryKey: qk.validationRuns(taskId),
    queryFn: () => apiFetch<ValidationRunResponse[]>(`/tasks/${taskId}/validation-runs`),
    enabled: Boolean(taskId),
  })
}

// --- Notifications ---

export function useNotificationsQuery(projectId?: string, read?: boolean) {
  return useQuery({
    queryKey: qk.notifications(projectId, read),
    queryFn: () => listNotifications({ project_id: projectId, read, limit: 20 }),
  })
}

export function useUnreadNotificationsCountQuery(projectId?: string) {
  return useQuery({
    queryKey: qk.notificationUnreadCount(projectId),
    queryFn: () => getUnreadNotificationCount(projectId),
  })
}

export function useMarkNotificationRead() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (notificationId: string) => markNotificationRead(notificationId),
    onSuccess: (notification) => {
      void queryClient.invalidateQueries({
        predicate: (query) =>
          String(query.queryKey[0]) === 'notifications' &&
          String(query.queryKey[1] ?? '') === notification.project_id,
      })
      void queryClient.invalidateQueries({
        queryKey: qk.notificationUnreadCount(notification.project_id),
      })
      void queryClient.invalidateQueries({ queryKey: qk.notificationUnreadCount() })
    },
  })
}

export function useMarkAllNotificationsRead() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectId?: string) => markAllNotificationsRead(projectId),
    onSuccess: (_, projectId) => {
      void queryClient.invalidateQueries({
        predicate: (query) =>
          String(query.queryKey[0]) === 'notifications' &&
          String(query.queryKey[1] ?? '') === (projectId ?? 'all'),
      })
      void queryClient.invalidateQueries({ queryKey: qk.notificationUnreadCount(projectId) })
      void queryClient.invalidateQueries({ queryKey: qk.notificationUnreadCount() })
    },
  })
}

export function useDeleteNotification() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ notificationId }: { notificationId: string }) =>
      deleteNotification(notificationId),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        predicate: (query) => String(query.queryKey[0]) === 'notifications',
      })
    },
  })
}

export function useTaskWorkspace(taskId: string) {
  return useQuery({
    queryKey: qk.taskWorkspace(taskId),
    queryFn: () => apiFetch<Workspace>(`/tasks/${taskId}/workspace`),
    enabled: Boolean(taskId),
    retry: false,
  })
}

export function useResetTaskWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (taskId: string) =>
      apiFetch<unknown>(`/tasks/${taskId}/workspace/reset`, {
        method: 'POST',
      }),
    onSuccess: (_, taskId) => {
      void queryClient.invalidateQueries({ queryKey: qk.task(taskId) })
    },
  })
}

export function useFollowUpExecution(executionId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: FollowUpRequest) =>
      apiFetch<Execution>(`/executions/${executionId}/follow-up`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: (nextExecution) => {
      void queryClient.invalidateQueries({ queryKey: qk.task(nextExecution.task_id) })
      void queryClient.invalidateQueries({
        queryKey: qk.projects,
        predicate: ({ queryKey }) => queryKey[2] === 'tasks',
      })
      void queryClient.invalidateQueries({ queryKey: qk.executions(nextExecution.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.execution(executionId) })
      void queryClient.invalidateQueries({ queryKey: qk.execution(nextExecution.id) })
      void queryClient.invalidateQueries({ queryKey: qk.executionLogs(nextExecution.id) })
      void queryClient.invalidateQueries({ queryKey: qk.taskDiff(nextExecution.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.agents })
    },
  })
}

export function useTaskDiffQuery(taskId: string) {
  return useQuery({
    queryKey: qk.taskDiff(taskId),
    queryFn: () => apiFetch<DiffEnvelope>(`/tasks/${taskId}/diff`),
    enabled: Boolean(taskId),
  })
}

// --- Agents ---

const AGENTS_CACHE_STALE_TIME = 10 * 60 * 1000
const AGENTS_CACHE_GC_TIME = 60 * 60 * 1000

type AgentPagesData = InfiniteData<PaginatedResponse<Agent>, string | undefined>

function isPaginatedAgents(data: unknown): data is PaginatedResponse<Agent> {
  return (
    typeof data === 'object' && data !== null && Array.isArray((data as { items?: unknown }).items)
  )
}

function isAgentPagesData(data: unknown): data is AgentPagesData {
  return (
    typeof data === 'object' && data !== null && Array.isArray((data as { pages?: unknown }).pages)
  )
}

function findCachedAgent(
  queryClient: ReturnType<typeof useQueryClient>,
  agentId: string,
): Agent | undefined {
  const cachedAgent = queryClient.getQueryData<Agent>(qk.agent(agentId))
  if (cachedAgent) return cachedAgent

  for (const [, data] of queryClient.getQueriesData<unknown>({ queryKey: qk.agents })) {
    if (isPaginatedAgents(data)) {
      const agent = data.items.find((item) => item.id === agentId)
      if (agent) return agent
    }

    if (isAgentPagesData(data)) {
      for (const page of data.pages) {
        const agent = page.items.find((item) => item.id === agentId)
        if (agent) return agent
      }
    }
  }

  return undefined
}

export function useAgentsQuery() {
  return useQuery({
    queryKey: qk.agents,
    queryFn: () => apiFetch<PaginatedResponse<Agent>>('/agents', { search: { limit: 100 } }),
    staleTime: AGENTS_CACHE_STALE_TIME,
    gcTime: AGENTS_CACHE_GC_TIME,
  })
}

export function useAgentsInfiniteQuery(limit = 20) {
  return useInfiniteQuery({
    queryKey: qk.agentPages(limit),
    queryFn: ({ pageParam }) =>
      apiFetch<PaginatedResponse<Agent>>('/agents', {
        search: { cursor: pageParam as string | undefined, limit, include_total: true },
      }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  })
}

export function useAgentQuery(agentId?: string) {
  const queryClient = useQueryClient()
  return useQuery({
    queryKey: qk.agent(agentId ?? ''),
    queryFn: () => apiFetch<Agent>(`/agents/${agentId}`),
    enabled: Boolean(agentId),
    placeholderData: () => (agentId ? findCachedAgent(queryClient, agentId) : undefined),
    gcTime: AGENTS_CACHE_GC_TIME,
  })
}

export function useCreateAgent() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateAgentRequest) =>
      apiFetch<Agent>('/agents', {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.agents })
    },
  })
}

export function useUpdateAgent() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ agentId, body }: { agentId: string; body: UpdateAgentRequest }) =>
      apiFetch<Agent>(`/agents/${agentId}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    onSuccess: (agent) => {
      void queryClient.invalidateQueries({ queryKey: qk.agent(agent.id) })
      void queryClient.invalidateQueries({ queryKey: qk.agents })
    },
  })
}

export function usePauseAgent() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (agentId: string) =>
      apiFetch<Agent>(`/agents/${agentId}/pause`, {
        method: 'POST',
      }),
    onSuccess: (agent) => {
      void queryClient.invalidateQueries({ queryKey: qk.agent(agent.id) })
      void queryClient.invalidateQueries({ queryKey: qk.agents })
    },
  })
}

export function useResumeAgent() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (agentId: string) =>
      apiFetch<Agent>(`/agents/${agentId}/resume`, {
        method: 'POST',
      }),
    onSuccess: (agent) => {
      void queryClient.invalidateQueries({ queryKey: qk.agent(agent.id) })
      void queryClient.invalidateQueries({ queryKey: qk.agents })
    },
  })
}

export function useDeleteAgent() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (agentId: string) =>
      apiFetch<void>(`/agents/${agentId}`, {
        method: 'DELETE',
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.agents })
    },
  })
}

export function useAgentDiscoveredOptions(agentId: string | null | undefined) {
  return useQuery({
    queryKey: qk.agentDiscoveredOptions(agentId ?? ''),
    queryFn: () => apiFetch<AgentDiscoveredOptions>(`/agents/${agentId}/discovered-options`),
    enabled: Boolean(agentId),
    staleTime: 5 * 60 * 1000,
  })
}

export function useAgentAvailability(agentId: string | null | undefined) {
  return useQuery({
    queryKey: qk.agentAvailability(agentId ?? ''),
    queryFn: () => apiFetch<AgentAvailability>(`/agents/${agentId}/availability`),
    enabled: Boolean(agentId),
    staleTime: 30 * 1000,
  })
}

export function useExecutorTypesQuery() {
  return useQuery({
    queryKey: qk.executorTypes,
    queryFn: () => apiFetch<ExecutorType[]>('/executor-types'),
  })
}

export function useDaemonsQuery(enabled = true) {
  return useQuery({
    queryKey: qk.daemons,
    queryFn: () => apiFetch<PaginatedResponse<Daemon>>('/daemons'),
    enabled,
  })
}

export function useOperationsStatusQuery() {
  return useQuery({
    queryKey: qk.operationsStatus,
    queryFn: getOperationsStatus,
    refetchInterval: 30_000,
  })
}

export function useRefreshOperationsMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: refreshOperations,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.operationsStatus })
    },
  })
}

export function useCliProjectionQuery() {
  return useQuery({
    queryKey: qk.clis,
    queryFn: () => apiFetch<CliProjectionResponse>('/clis'),
  })
}

function browserPublicBaseUrl(): string | undefined {
  if (typeof window === 'undefined') return undefined
  return window.location.origin
}

export function useMcpConfigQuery(agent: string, scope: string, projectId?: string) {
  return useQuery({
    queryKey: qk.mcpConfig(agent, scope, projectId),
    queryFn: () =>
      apiFetch<McpConfigResponse>('/config/mcp', {
        search: {
          agent,
          scope,
          public_base_url: browserPublicBaseUrl(),
          ...(projectId ? { project_id: projectId } : {}),
        },
      }),
  })
}

export function useUpdateMcpConfig() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: McpConfigActionRequest) =>
      apiFetch<McpConfigResponse>('/config/mcp', {
        method: 'POST',
        body: JSON.stringify({
          ...body,
          public_base_url: body.public_base_url ?? browserPublicBaseUrl(),
        }),
      }),
    onSuccess: (_data, variables) => {
      void queryClient.invalidateQueries({
        queryKey: qk.mcpConfig(
          variables.agent,
          variables.scope ?? 'project',
          variables.project_id ?? undefined,
        ),
      })
    },
  })
}

// --- Repos ---

export function useReposQuery(projectId: string) {
  return useQuery({
    queryKey: qk.repos(projectId),
    queryFn: () => apiFetch<PaginatedResponse<Repo>>(`/projects/${projectId}/repos`),
  })
}

export function useCreateRepo(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: CreateRepoRequest) =>
      apiFetch<Repo>(`/projects/${projectId}/repos`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.repos(projectId) })
      void queryClient.invalidateQueries({ queryKey: qk.project(projectId) })
      void queryClient.invalidateQueries({ queryKey: qk.projects })
    },
  })
}

export function useUpdateRepo(repoId: string, projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: UpdateRepoRequest) =>
      apiFetch<Repo>(`/repos/${repoId}`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.repos(projectId) })
      void queryClient.invalidateQueries({ queryKey: qk.project(projectId) })
    },
  })
}

export function useSyncRepo(repoId: string, projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () =>
      apiFetch<RepoSyncResponse>(`/repos/${repoId}/sync`, {
        method: 'POST',
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.repos(projectId) })
    },
  })
}

// --- Executions ---

export function useExecutionsQuery(taskId: string) {
  return useQuery({
    queryKey: qk.executions(taskId),
    queryFn: () => apiFetch<PaginatedResponse<Execution>>(`/tasks/${taskId}/executions`),
  })
}

export function useStartExecution() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ taskId, body }: { taskId: string; body: StartExecutionRequest }) =>
      apiFetch<Execution>(`/tasks/${taskId}/executions`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: (execution) => {
      void queryClient.invalidateQueries({ queryKey: qk.executions(execution.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.task(execution.task_id) })
    },
  })
}

export function useExecutionQuery(executionId: string) {
  return useQuery({
    queryKey: qk.execution(executionId),
    queryFn: () => apiFetch<Execution>(`/executions/${executionId}`),
    enabled: Boolean(executionId),
  })
}

export function useExecutionUsageQuery(executionId: string) {
  return useQuery({
    queryKey: qk.executionUsage(executionId),
    queryFn: () => apiFetch<ExecutionUsage[]>(`/executions/${executionId}/usage`),
  })
}

export function useTaskUsageQuery(taskId: string) {
  return useQuery({
    queryKey: qk.taskUsage(taskId),
    queryFn: () => apiFetch<TaskUsageSummary>(`/tasks/${taskId}/usage`),
  })
}

/**
 * Execution logs are returned as parsed JSONL entries from the backend.
 */
export async function getExecutionLogs(
  executionId: string,
  params?: ExecutionLogsParams,
): Promise<ExecutionLogsResponse> {
  const data = await apiFetch<ExecutionLogsResponse>(`/executions/${executionId}/logs`, {
    search: {
      tail: params?.tail,
      from_sequence: params?.from_sequence,
      limit: params?.limit,
    },
  })
  const response: ExecutionLogsResponse = {
    items: data.items ?? [],
    has_more: data.has_more ?? false,
    next_sequence: data.next_sequence ?? null,
  }
  return response
}

export function useExecutionLogs(
  executionId: string,
  params?: ExecutionLogsParams,
  isRunning?: boolean,
) {
  return useQuery({
    queryKey: [...qk.executionLogs(executionId), params ?? {}] as const,
    enabled: Boolean(executionId),
    refetchInterval: isRunning ? 3000 : false,
    queryFn: () => getExecutionLogs(executionId, params),
  })
}

export interface HookLogEntry {
  event: string
  hook_type: string
  command?: string
  plugin_name?: string
  duration_ms: number
  status: string
  exit_code?: number | null
  timeout?: boolean
  working_dir?: string
  stdout?: string
  stderr?: string
  error?: string
  reason?: string
}

export function getExecutionHookLogs(executionId: string): Promise<HookLogEntry[]> {
  return apiFetch<HookLogEntry[]>(`/executions/${executionId}/hook-logs`)
}

export function useExecutionHookLogs(executionId: string | undefined) {
  return useQuery({
    queryKey: qk.executionHookLogs(executionId ?? ''),
    enabled: !!executionId,
    queryFn: () => getExecutionHookLogs(executionId ?? ''),
  })
}

export function useSettingsQuery() {
  return useQuery({
    queryKey: qk.settings,
    queryFn: () => getSettings(),
  })
}

export function useUpdateSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: UpdateSettingsRequest) => updateSettings(body),
    onSuccess: (data: SettingsResponse) => {
      queryClient.setQueryData(qk.settings, data)
    },
  })
}

// --- Integrations ---

export type SyncTriggerResponse = {
  imported: number
  skipped: number
  errors: number
}

export function useIntegrationQuery(projectId: string) {
  return useQuery({
    queryKey: qk.integration(projectId),
    queryFn: () => apiFetch<IntegrationResponse | null>(`/projects/${projectId}/integration`),
    retry: (failureCount, error) => {
      if (error instanceof ApiError && error.status === 404) return false
      return failureCount < 3
    },
  })
}

export function useCreateIntegration() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ projectId, body }: { projectId: string; body: CreateIntegrationRequest }) =>
      apiFetch<IntegrationResponse>(`/projects/${projectId}/integration`, {
        method: 'POST',
        body: JSON.stringify(body),
      }),
    onSuccess: (_, vars) => {
      void queryClient.invalidateQueries({ queryKey: qk.integration(vars.projectId) })
    },
  })
}

export function useUpdateIntegration() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ projectId, body }: { projectId: string; body: PatchIntegrationRequest }) =>
      apiFetch<IntegrationResponse>(`/projects/${projectId}/integration`, {
        method: 'PATCH',
        body: JSON.stringify(body),
      }),
    onSuccess: (_, vars) => {
      void queryClient.invalidateQueries({ queryKey: qk.integration(vars.projectId) })
    },
  })
}

export function useDeleteIntegration() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectId: string) =>
      apiFetch<void>(`/projects/${projectId}/integration`, { method: 'DELETE' }),
    onSuccess: (_, projectId) => {
      void queryClient.invalidateQueries({ queryKey: qk.integration(projectId) })
    },
  })
}

export function useTriggerSync() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (projectId: string) =>
      apiFetch<SyncTriggerResponse>(`/projects/${projectId}/integration/sync`, {
        method: 'POST',
      }),
    onSuccess: (_, projectId) => {
      void queryClient.invalidateQueries({ queryKey: qk.integration(projectId) })
    },
  })
}

// --- External Links ---

export type ExternalLinkResponse = {
  id: string
  task_id: string
  integration_id: string
  platform: string
  remote_owner: string
  remote_repo: string
  remote_issue_number: number
  remote_url: string
  global_id: string
  synced_at: string
  created_at: string
  updated_at: string
}

export function useExternalLinksQuery(taskId: string) {
  return useQuery({
    queryKey: qk.taskExternalLinks(taskId),
    queryFn: () => apiFetch<ExternalLinkResponse[]>(`/tasks/${taskId}/external-links`),
    enabled: Boolean(taskId),
  })
}

export function useCreateExternalLink() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ taskId, remoteIssueNumber }: { taskId: string; remoteIssueNumber: number }) =>
      apiFetch<ExternalLinkResponse>(`/tasks/${taskId}/external-links`, {
        method: 'POST',
        body: JSON.stringify({ remote_issue_number: remoteIssueNumber }),
      }),
    onSuccess: (link) => {
      void queryClient.invalidateQueries({ queryKey: qk.taskExternalLinks(link.task_id) })
      void queryClient.invalidateQueries({ queryKey: qk.task(link.task_id) })
    },
  })
}

export function useDeleteExternalLink() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ taskId, linkId }: { taskId: string; linkId: string }) =>
      apiFetch<void>(`/tasks/${taskId}/external-links/${linkId}`, {
        method: 'DELETE',
      }),
    onSuccess: (_, vars) => {
      void queryClient.invalidateQueries({ queryKey: qk.taskExternalLinks(vars.taskId) })
      void queryClient.invalidateQueries({ queryKey: qk.task(vars.taskId) })
    },
  })
}

export function useTaskDependenciesQuery(taskId: string) {
  return useQuery({
    queryKey: qk.taskDependencies(taskId),
    queryFn: () => listDependencies(taskId),
    enabled: Boolean(taskId),
  })
}

export function useTaskDependentsQuery(taskId: string) {
  return useQuery({
    queryKey: qk.taskDependents(taskId),
    queryFn: () => listDependents(taskId),
    enabled: Boolean(taskId),
  })
}

export function useAddDependency(taskId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (dependsOnId: string) => addDependency(taskId, dependsOnId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.taskDependencies(taskId) })
    },
  })
}

export function useRemoveDependency(taskId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (dependsOnId: string) => removeDependency(taskId, dependsOnId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.taskDependencies(taskId) })
    },
  })
}

// --- User Profile ---

export function useUpdateMe() {
  return useMutation({
    mutationFn: (body: { email?: string | null; display_name?: string | null }) => updateMe(body),
    onSuccess: (user) => useAuthStore.getState().updateUser(user),
  })
}

// --- Personal Access Tokens ---

export function usePatsQuery() {
  return useQuery({
    queryKey: qk.pats(),
    queryFn: () => listPats(),
  })
}

export function useCreatePat() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { name: string; expires_at?: string | null }) => createPat(body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.pats() })
    },
  })
}

export function useDeletePat() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => deletePat(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.pats() })
    },
  })
}

// --- User Search ---

export function useUserSearch(query: string) {
  return useQuery({
    queryKey: ['users', 'search', query],
    queryFn: () => searchUsers(query),
    enabled: query.trim().length >= 2,
    staleTime: 30_000,
  })
}

// --- Project Members ---

export function useMembersQuery(projectId: string) {
  return useQuery({
    queryKey: qk.projectMembers(projectId),
    queryFn: () => listMembers(projectId),
    enabled: Boolean(projectId),
  })
}

export function useAddMember(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { user_id: string; role: string }) => addMember(projectId, body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.projectMembers(projectId) })
    },
  })
}

export function useUpdateMemberRole(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ userId, body }: { userId: string; body: { role: string } }) =>
      updateMemberRole(projectId, userId, body),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.projectMembers(projectId) })
    },
  })
}

export function useRemoveMember(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (userId: string) => removeMember(projectId, userId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.projectMembers(projectId) })
    },
  })
}
