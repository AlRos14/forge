import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { qk } from '@/api/query-keys'
import {
  applyProjectPlan,
  authorizeProjectPlanBatch,
  getProjectPlan,
  updateProjectPlanBatch,
} from './api'
import type {
  ApplyProjectPlanRequest,
  AuthorizeProjectPlanBatchRequest,
  UpdateProjectPlanBatchRequest,
} from '@/types/generated'

export const projectPlanQueryKey = (projectId: string) => ['projects', projectId, 'plan'] as const

export function useProjectPlanQuery(projectId: string) {
  return useQuery({
    queryKey: projectPlanQueryKey(projectId),
    queryFn: () => getProjectPlan(projectId),
    enabled: Boolean(projectId),
    refetchInterval: 15_000,
  })
}

export function useProjectPlanMutations(projectId: string) {
  const queryClient = useQueryClient()
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: projectPlanQueryKey(projectId) })
    void queryClient.invalidateQueries({ queryKey: qk.projectTasks(projectId) })
  }
  const apply = useMutation({
    mutationFn: ({ proposalId, body }: { proposalId: string; body: ApplyProjectPlanRequest }) =>
      applyProjectPlan(projectId, proposalId, body),
    onSuccess: refresh,
    onError: refresh,
  })
  const authorize = useMutation({
    mutationFn: (body: AuthorizeProjectPlanBatchRequest) =>
      authorizeProjectPlanBatch(projectId, body),
    onSuccess: refresh,
    onError: refresh,
  })
  const updateBatch = useMutation({
    mutationFn: ({ batchId, body }: { batchId: string; body: UpdateProjectPlanBatchRequest }) =>
      updateProjectPlanBatch(projectId, batchId, body),
    onSuccess: refresh,
    onError: refresh,
  })
  return { apply, authorize, updateBatch }
}
