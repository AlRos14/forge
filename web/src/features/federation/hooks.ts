import { useEffect } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ApiError } from '@/api/client'
import {
  cancelProviderAuthorization,
  createProviderEntry,
  getAgentUsage,
  getProviderAuthorization,
  getProviderUsage,
  listAgentProfiles,
  listAgentProviderCapabilities,
  listFederatedAgents,
  listProviders,
  registerHarnessAgent,
  removeProviderEntry,
  renameProviderEntry,
  selectAgentProfile,
  startProviderAuthorization,
} from './api'
import type {
  CancelProviderAuthorizationRequest,
  CreateProviderEntryRequest,
  RenameProviderEntryRequest,
  StartProviderAuthorizationRequest,
} from '@/types/generated'

export const federationQueryKeys = {
  agents: ['federated-agents'] as const,
  profiles: (identityId: string) => ['federated-agents', identityId, 'profiles'] as const,
  usage: (identityId: string) => ['federated-agents', identityId, 'usage'] as const,
  credentials: ['federated-agents', 'credentials'] as const,
  providers: ['agent-providers'] as const,
  providerUsage: (id: string) => ['agent-providers', id, 'usage'] as const,
  providerAuthorization: (id: string) => ['provider-authorizations', id] as const,
} as const

export function useFederatedAgentsQuery() {
  return useQuery({
    queryKey: federationQueryKeys.agents,
    queryFn: () => listFederatedAgents(),
    staleTime: 10_000,
  })
}

export function useAgentProfilesQuery(identityId: string | undefined) {
  return useQuery({
    queryKey: federationQueryKeys.profiles(identityId ?? 'none'),
    queryFn: () => listAgentProfiles(identityId!),
    enabled: Boolean(identityId),
    staleTime: 15_000,
  })
}

export function useAgentUsageQuery(identityId: string | undefined) {
  return useQuery({
    queryKey: federationQueryKeys.usage(identityId ?? 'none'),
    queryFn: () => getAgentUsage(identityId!),
    enabled: Boolean(identityId),
    staleTime: 60_000,
  })
}

export function useSelectAgentProfileMutation(identityId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ profileId, version }: { profileId: string; version: number }) =>
      selectAgentProfile(identityId, profileId, version),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.profiles(identityId) })
    },
  })
}

export function useRegisterHarnessAgentMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: Parameters<typeof registerHarnessAgent>[0]) => registerHarnessAgent(input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.credentials })
    },
  })
}

export function useProvidersQuery() {
  return useQuery({
    queryKey: federationQueryKeys.credentials,
    queryFn: listProviders,
    staleTime: 15_000,
  })
}

export function useCreateProviderEntryMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: CreateProviderEntryRequest) => createProviderEntry(input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.credentials })
    },
  })
}

export function useRenameProviderEntryMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: RenameProviderEntryRequest }) =>
      renameProviderEntry(id, input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.credentials })
    },
  })
}

export function useAgentProviderCapabilitiesQuery() {
  return useQuery({
    queryKey: federationQueryKeys.providers,
    queryFn: listAgentProviderCapabilities,
    staleTime: 60_000,
  })
}

export function useProviderUsageQuery(entryId: string | undefined) {
  return useQuery({
    queryKey: federationQueryKeys.providerUsage(entryId ?? 'none'),
    queryFn: () => getProviderUsage(entryId!),
    enabled: Boolean(entryId),
    staleTime: 3 * 60_000,
    retry: false,
  })
}

export function useProviderAuthorizationQuery(id: string | undefined) {
  const queryClient = useQueryClient()
  const query = useQuery({
    queryKey: federationQueryKeys.providerAuthorization(id ?? 'none'),
    queryFn: () => getProviderAuthorization(id!),
    enabled: Boolean(id),
    refetchInterval: (query) => {
      const state = query.state.data?.state
      return state && ['succeeded', 'denied', 'expired', 'cancelled', 'failed'].includes(state)
        ? false
        : 1500
    },
  })
  const succeeded = query.data?.state === 'succeeded'
  useEffect(() => {
    if (!succeeded) return
    void queryClient.invalidateQueries({ queryKey: federationQueryKeys.credentials })
    void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
  }, [succeeded, queryClient])
  return query
}

export function useStartProviderAuthorizationMutation() {
  return useMutation({
    mutationFn: (input: StartProviderAuthorizationRequest) => startProviderAuthorization(input),
  })
}

export function useCancelProviderAuthorizationMutation() {
  return useMutation({
    mutationFn: ({ id, input }: { id: string; input: CancelProviderAuthorizationRequest }) =>
      cancelProviderAuthorization(id, input),
  })
}

export function useRemoveProviderEntryMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ handleId, version }: { handleId: string; version: number }) =>
      removeProviderEntry(handleId, version),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.credentials })
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
    },
  })
}

export function isVersionConflict(error: unknown): boolean {
  return error instanceof ApiError && error.status === 409
}
