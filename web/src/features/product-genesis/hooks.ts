import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  cancelProductGenesis,
  getActiveProductGenesis,
  readyProductGenesis,
  startProductGenesis,
} from './api'
import type {
  ProductGenesisCancelInput,
  ProductGenesisReadyInput,
  ProductGenesisStartInput,
} from './types'

export const productGenesisQueryKeys = {
  active: ['product-genesis', 'active'] as const,
} as const

export function useProductGenesisActiveQuery() {
  return useQuery({
    queryKey: productGenesisQueryKeys.active,
    queryFn: getActiveProductGenesis,
    staleTime: 3_000,
    // Genesis lifecycle transitions are server-side events. Keep the status
    // chip current even when the backend has no SSE event for the transition.
    refetchInterval: 2_000,
  })
}

function invalidateProductGenesis(queryClient: ReturnType<typeof useQueryClient>) {
  void queryClient.invalidateQueries({ queryKey: productGenesisQueryKeys.active })
  // Genesis admits its first turn into the existing Main Chat.  Invalidate
  // the chat prefix so the timeline and switcher never require a second chat.
  void queryClient.invalidateQueries({ queryKey: ['agent-chats'] })
}

export function useStartProductGenesisMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: ProductGenesisStartInput) => startProductGenesis(input),
    onSuccess: () => invalidateProductGenesis(queryClient),
  })
}

export function useCancelProductGenesisMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ sessionId, input }: { sessionId: string; input: ProductGenesisCancelInput }) =>
      cancelProductGenesis(sessionId, input),
    onSuccess: () => invalidateProductGenesis(queryClient),
  })
}

export function useReadyProductGenesisMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ sessionId, input }: { sessionId: string; input: ProductGenesisReadyInput }) =>
      readyProductGenesis(sessionId, input),
    onSuccess: () => invalidateProductGenesis(queryClient),
  })
}
