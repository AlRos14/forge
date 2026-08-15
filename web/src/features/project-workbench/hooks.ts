import { useMutation, useQueryClient } from '@tanstack/react-query'
import { qk } from '@/api/query-keys'
import type { UserResponse } from '@/types/generated'
import {
  createWorkbenchDecision,
  createWorkbenchDocument,
  createWorkbenchMilestone,
} from './api'

function useProjectRecordMutation<TInput, TOutput>(
  projectId: string,
  mutationFn: (input: TInput) => Promise<TOutput>,
) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: qk.project(projectId) })
      void queryClient.invalidateQueries({ queryKey: qk.projectOverview(projectId) })
    },
  })
}

export function useCreateWorkbenchDocument(
  projectId: string,
  user: UserResponse | null,
  projectVersion: number,
) {
  return useProjectRecordMutation(
    projectId,
    (input: Parameters<typeof createWorkbenchDocument>[3]) => {
      if (!user) throw new Error('An authenticated user is required to edit Project records.')
      return createWorkbenchDocument(projectId, user, projectVersion, input)
    },
  )
}

export function useCreateWorkbenchDecision(
  projectId: string,
  user: UserResponse | null,
  projectVersion: number,
) {
  return useProjectRecordMutation(
    projectId,
    (input: Parameters<typeof createWorkbenchDecision>[3]) => {
      if (!user) throw new Error('An authenticated user is required to edit Project records.')
      return createWorkbenchDecision(projectId, user, projectVersion, input)
    },
  )
}

export function useCreateWorkbenchMilestone(
  projectId: string,
  user: UserResponse | null,
  projectVersion: number,
) {
  return useProjectRecordMutation(
    projectId,
    (input: Parameters<typeof createWorkbenchMilestone>[3]) => {
      if (!user) throw new Error('An authenticated user is required to edit Project records.')
      return createWorkbenchMilestone(projectId, user, projectVersion, input)
    },
  )
}
