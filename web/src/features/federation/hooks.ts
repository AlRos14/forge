import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ApiError } from '@/api/client'
import {
  cancelAgentSessionTurn,
  connectEmbeddedAgent,
  createAgentSession,
  getEffectivePermissions,
  getContextManifest,
  getMainAgentBinding,
  getMissionControl,
  listContextManifests,
  listAgentProfiles,
  listAgentSessions,
  listCredentials,
  listFederatedAgents,
  getProjectAgentBinding,
  revokeCredential,
  rotateAgentSession,
  selectAgentProfile,
  setMainAgentBinding,
  setProjectAgentBinding,
  setAgentSessionStatus,
  steerAgentSessionTurn,
} from './api'
import type {
  ConnectEmbeddedAgentInput,
  ContextManifestLookup,
  CreateAgentSessionInput,
  ProjectAgentBindingInput,
} from './types'

export const federationQueryKeys = {
  agents: ['federated-agents'] as const,
  profiles: (identityId: string) => ['federated-agents', identityId, 'profiles'] as const,
  sessions: (identityId: string) => ['federated-agents', identityId, 'sessions'] as const,
  credentials: ['federated-agents', 'credentials'] as const,
  mainAgent: ['federated-agents', 'main-agent'] as const,
  missionControl: ['mission-control'] as const,
  contextManifest: (manifestId: string, identityId: string, contextScopeId: string) =>
    ['context-manifests', manifestId, identityId, contextScopeId] as const,
  contextManifestDiscovery: (identityId: string, contextScopeId: string) =>
    ['context-manifests', 'discovery', identityId, contextScopeId] as const,
  projectAgent: (projectId: string) => ['projects', projectId, 'project-agent'] as const,
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

export function useAgentSessionsQuery(identityId: string | undefined) {
  return useQuery({
    queryKey: federationQueryKeys.sessions(identityId ?? 'none'),
    queryFn: () => listAgentSessions(identityId!),
    enabled: Boolean(identityId),
    staleTime: 5_000,
  })
}

export function useConnectEmbeddedAgentMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: ConnectEmbeddedAgentInput) => connectEmbeddedAgent(input),
    onSuccess: (connected) => {
      queryClient.setQueryData(
        federationQueryKeys.agents,
        (current: { items: unknown[] } | undefined) => {
          if (!current) return current
          return {
            ...current,
            items: [
              connected.agent,
              ...current.items.filter(
                (item) => (item as { id?: string }).id !== connected.agent.id,
              ),
            ],
          }
        },
      )
      queryClient.setQueryData(federationQueryKeys.profiles(connected.agent.id), [
        connected.profile,
      ])
      queryClient.setQueryData(federationQueryKeys.sessions(connected.agent.id), [
        connected.session,
      ])
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
    },
  })
}

export function useCreateAgentSessionMutation(identityId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: CreateAgentSessionInput) => createAgentSession(identityId, input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.sessions(identityId) })
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
    },
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

export function useRotateAgentSessionMutation(identityId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ sessionId, version }: { sessionId: string; version: number }) =>
      rotateAgentSession(sessionId, version),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.sessions(identityId) })
    },
  })
}

export function useSetAgentSessionStatusMutation(identityId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      sessionId,
      status,
      version,
    }: {
      sessionId: string
      status: 'suspend' | 'resume'
      version: number
    }) => setAgentSessionStatus(sessionId, status, version),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.sessions(identityId) })
    },
  })
}

export function useCancelAgentSessionMutation(identityId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (sessionId: string) => cancelAgentSessionTurn(sessionId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.sessions(identityId) })
    },
  })
}

export function useSteerAgentSessionMutation(identityId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ sessionId, content }: { sessionId: string; content: string }) =>
      steerAgentSessionTurn(sessionId, content),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.sessions(identityId) })
    },
  })
}

export function useCredentialsQuery() {
  return useQuery({
    queryKey: federationQueryKeys.credentials,
    queryFn: listCredentials,
    staleTime: 15_000,
  })
}

export function useMainAgentBindingQuery() {
  return useQuery({
    queryKey: federationQueryKeys.mainAgent,
    queryFn: getMainAgentBinding,
    staleTime: 10_000,
  })
}

export function useSetMainAgentBindingMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setMainAgentBinding,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.mainAgent })
      void queryClient.invalidateQueries({ queryKey: ['agent-chats'] })
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
    },
  })
}

export function useRevokeCredentialMutation() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (handleId: string) => revokeCredential(handleId),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.credentials })
    },
  })
}

export function useEffectivePermissionsQuery(
  identityId: string | undefined,
  scope: CreateAgentSessionInput['scope'] | undefined,
) {
  return useQuery({
    queryKey: ['federated-agents', identityId ?? 'none', 'permissions', scope],
    queryFn: () => getEffectivePermissions(identityId!, scope!),
    enabled: Boolean(identityId && scope),
    staleTime: 15_000,
  })
}

export function useMissionControlQuery() {
  return useQuery({
    queryKey: federationQueryKeys.missionControl,
    queryFn: getMissionControl,
    staleTime: 15_000,
    refetchInterval: 30_000,
  })
}

export function useContextManifestQuery(
  lookup: (ContextManifestLookup & { manifest_id: string }) | undefined,
) {
  return useQuery({
    queryKey: federationQueryKeys.contextManifest(
      lookup?.manifest_id ?? 'none',
      lookup?.identity_id ?? 'none',
      lookup?.context_scope_id ?? 'none',
    ),
    queryFn: () => {
      const { manifest_id, ...query } = lookup!
      return getContextManifest(manifest_id, query)
    },
    enabled: Boolean(lookup?.manifest_id && lookup.identity_id && lookup.context_scope_id),
    staleTime: 30_000,
  })
}

export function useContextManifestDiscoveryQuery(lookup: ContextManifestLookup | undefined) {
  return useQuery({
    queryKey: federationQueryKeys.contextManifestDiscovery(
      lookup?.identity_id ?? 'none',
      lookup?.context_scope_id ?? 'none',
    ),
    queryFn: () => listContextManifests(lookup!),
    enabled: Boolean(lookup?.identity_id && lookup.context_scope_id),
    staleTime: 30_000,
  })
}

export function useProjectAgentBindingQuery(projectId: string | undefined) {
  return useQuery({
    queryKey: federationQueryKeys.projectAgent(projectId ?? 'none'),
    queryFn: () => getProjectAgentBinding(projectId!),
    enabled: Boolean(projectId),
    staleTime: 10_000,
  })
}

export function useSetProjectAgentBindingMutation(projectId: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: ProjectAgentBindingInput) => setProjectAgentBinding(projectId, input),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.projectAgent(projectId) })
      void queryClient.invalidateQueries({ queryKey: ['agent-chats'] })
      void queryClient.invalidateQueries({ queryKey: federationQueryKeys.agents })
    },
  })
}

export function isVersionConflict(error: unknown): boolean {
  return error instanceof ApiError && error.status === 409
}
