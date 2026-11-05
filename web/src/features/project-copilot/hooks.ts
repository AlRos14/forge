import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  getProjectCopilot,
  getProjectCopilotRun,
  cancelProjectCopilotRun,
  listProjectCopilotActivity,
  listProjectCopilotMessages,
  sendProjectCopilotMessage,
  updateProjectCopilot,
} from '@/features/project-copilot/api'
import type {
  SendProjectCopilotMessageRequest,
  UpdateProjectCopilotRequest,
} from '@/features/project-copilot/types'
import { isProjectCopilotRunPending } from '@/features/project-copilot/types'

export const projectCopilotQueryKeys = {
  config: (projectId: string) => ['projects', projectId, 'copilot'] as const,
  activity: (projectId: string) => ['projects', projectId, 'copilot', 'activity'] as const,
  messages: (projectId: string) => ['projects', projectId, 'copilot', 'messages'] as const,
  run: (projectId: string, runId: string) =>
    ['projects', projectId, 'copilot', 'runs', runId] as const,
}

export function useProjectCopilotActivityQuery(projectId: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: projectCopilotQueryKeys.activity(projectId),
    queryFn: ({ pageParam }) =>
      listProjectCopilotActivity(projectId, pageParam as string | undefined, 30),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
    enabled: Boolean(projectId) && enabled,
    refetchInterval: enabled ? 5_000 : false,
  })
}

export function useProjectCopilotQuery(projectId: string, enabled: boolean) {
  return useQuery({
    queryKey: projectCopilotQueryKeys.config(projectId),
    queryFn: () => getProjectCopilot(projectId),
    enabled: Boolean(projectId) && enabled,
  })
}

export function useProjectCopilotMessagesQuery(projectId: string, enabled: boolean) {
  return useInfiniteQuery({
    queryKey: projectCopilotQueryKeys.messages(projectId),
    queryFn: ({ pageParam }) =>
      listProjectCopilotMessages(projectId, pageParam as string | undefined, 50),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
    enabled: Boolean(projectId) && enabled,
  })
}

export function useUpdateProjectCopilotMutation(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: UpdateProjectCopilotRequest) => updateProjectCopilot(projectId, body),
    onSuccess: (config) => {
      queryClient.setQueryData(projectCopilotQueryKeys.config(projectId), config)
    },
  })
}

export function useSendProjectCopilotMessageMutation(projectId: string) {
  return useMutation({
    mutationFn: (body: SendProjectCopilotMessageRequest) =>
      sendProjectCopilotMessage(projectId, body),
  })
}

export function useCancelProjectCopilotRunMutation(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (runId: string) => cancelProjectCopilotRun(projectId, runId),
    onSuccess: (run) => {
      void queryClient.invalidateQueries({
        queryKey: projectCopilotQueryKeys.run(projectId, run.id),
      })
      void queryClient.invalidateQueries({ queryKey: projectCopilotQueryKeys.config(projectId) })
      void queryClient.invalidateQueries({ queryKey: projectCopilotQueryKeys.messages(projectId) })
      void queryClient.invalidateQueries({ queryKey: projectCopilotQueryKeys.activity(projectId) })
    },
  })
}

export function useProjectCopilotRunQuery(
  projectId: string,
  runId: string | null,
  enabled: boolean,
) {
  return useQuery({
    queryKey: projectCopilotQueryKeys.run(projectId, runId ?? 'none'),
    queryFn: () => getProjectCopilotRun(projectId, runId!),
    enabled: Boolean(projectId && runId) && enabled,
    refetchInterval: (query) =>
      isProjectCopilotRunPending(query.state.data?.run.status) ? 1_500 : false,
  })
}
