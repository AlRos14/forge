import { apiFetch } from '@/api/client'
import type {
  AgentConnectionHealth,
  AgentProfile,
  AgentSession,
  AttentionItem,
  ConnectEmbeddedAgentInput,
  ContextManifest,
  ContextManifestDiscoveryQuery,
  ContextManifestDiscoveryResponse,
  ContextManifestLookup,
  ConnectedEmbeddedAgent,
  CreateAgentSessionInput,
  CredentialHandle,
  EffectivePermissions,
  FederatedAgent,
  MissionControlResponse,
  MainAgentBinding,
  MainAgentBindingInput,
  Page,
  ProjectAgentBinding,
  ProjectAgentBindingInput,
} from './types'

/**
 * Federation API adapter.
 *
 * Embedded-agent routes are live in the current backend migration.
 * Mission Control is intentionally kept behind this adapter because its
 * read-model route is owned by the attention projection and may be unavailable
 * while a server is upgrading.
 * The UI treats a missing projection as a recoverable error instead of
 * synthesizing authoritative state in the browser.
 */

export const federationApiPaths = {
  missionControl: '/mission-control',
} as const

export function listFederatedAgents(limit = 100, cursor?: string): Promise<Page<FederatedAgent>> {
  return apiFetch<Page<FederatedAgent>>('/agents', {
    search: { limit, cursor },
  })
}

export function listAgentProfiles(identityId: string): Promise<AgentProfile[]> {
  return apiFetch<AgentProfile[]>(`/agents/${identityId}/profiles`)
}

export function connectEmbeddedAgent(
  input: ConnectEmbeddedAgentInput,
): Promise<ConnectedEmbeddedAgent> {
  return apiFetch<ConnectedEmbeddedAgent>('/embedded-agents/connect', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function createAgentSession(
  identityId: string,
  input: CreateAgentSessionInput,
): Promise<AgentSession> {
  return apiFetch<AgentSession>(`/agents/${identityId}/sessions`, {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function listAgentSessions(identityId: string): Promise<AgentSession[]> {
  return apiFetch<AgentSession[]>(`/agents/${identityId}/sessions`)
}

export function selectAgentProfile(
  identityId: string,
  profileId: string,
  version: number,
): Promise<FederatedAgent> {
  return apiFetch<FederatedAgent>(`/agents/${identityId}/profiles/${profileId}/select`, {
    method: 'POST',
    body: JSON.stringify({ version }),
  })
}

export function rotateAgentSession(sessionId: string, version: number): Promise<AgentSession> {
  return apiFetch<AgentSession>(`/agent-sessions/${sessionId}/rotate`, {
    method: 'POST',
    body: JSON.stringify({ version }),
  })
}

export function setAgentSessionStatus(
  sessionId: string,
  status: 'suspend' | 'resume',
  version: number,
): Promise<AgentSession> {
  return apiFetch<AgentSession>(`/agent-sessions/${sessionId}/${status}`, {
    method: 'POST',
    body: JSON.stringify({ version }),
  })
}

export function cancelAgentSessionTurn(sessionId: string): Promise<void> {
  return apiFetch<void>(`/agent-sessions/${sessionId}/cancel`, { method: 'POST' })
}

export function steerAgentSessionTurn(sessionId: string, content: string): Promise<void> {
  return apiFetch<void>(`/agent-sessions/${sessionId}/steer`, {
    method: 'POST',
    body: JSON.stringify({ content }),
  })
}

export function listCredentials(): Promise<CredentialHandle[]> {
  return apiFetch<CredentialHandle[]>('/credentials')
}

export function revokeCredential(handleId: string): Promise<void> {
  return apiFetch<void>(`/credentials/${handleId}`, { method: 'DELETE' })
}

export function getAgentConnectionHealth(
  identityId: string,
  profileId: string,
): Promise<AgentConnectionHealth> {
  // Health is included in the connection response today. Keep the adapter
  // seam for the detail projection rather than exposing protected state.
  void identityId
  void profileId
  return Promise.reject(new Error('Connection health is returned with the profile projection'))
}

export function getEffectivePermissions(
  identityId: string,
  scope: CreateAgentSessionInput['scope'],
): Promise<EffectivePermissions> {
  return apiFetch<EffectivePermissions>(`/agents/${identityId}/effective-permissions`, {
    method: 'POST',
    body: JSON.stringify(scope),
  })
}

export function getMissionControl(): Promise<MissionControlResponse> {
  return apiFetch<MissionControlResponse>(federationApiPaths.missionControl)
}

export function getContextManifest(
  manifestId: string,
  query: ContextManifestLookup,
): Promise<ContextManifest> {
  return apiFetch<ContextManifest>(`/context-manifests/${manifestId}`, {
    search: query,
  })
}

/**
 * Discovery is scoped by identity in the route and optionally narrowed to a
 * context scope. The UI never fabricates a manifest id; it only selects ids
 * returned by the authorized response.
 */
export async function listContextManifests(
  query: ContextManifestLookup,
): Promise<ContextManifest[]> {
  const discoveryQuery: ContextManifestDiscoveryQuery = {
    context_scope_id: query.context_scope_id,
    limit: 50,
  }
  const response = await apiFetch<ContextManifestDiscoveryResponse>(
    `/agents/${query.identity_id}/context-manifests`,
    {
      search: {
        context_scope_id: discoveryQuery.context_scope_id ?? undefined,
        limit: discoveryQuery.limit ?? undefined,
      },
    },
  )
  return response.items
}

export function getProjectAgentBinding(projectId: string): Promise<ProjectAgentBinding> {
  return apiFetch<ProjectAgentBinding>(`/projects/${projectId}/project-agent`)
}

export function getMainAgentBinding(): Promise<MainAgentBinding> {
  return apiFetch<MainAgentBinding>('/account/main-agent')
}

export function setMainAgentBinding(input: MainAgentBindingInput): Promise<MainAgentBinding> {
  return apiFetch<MainAgentBinding>('/account/main-agent', {
    method: 'PUT',
    body: JSON.stringify(input),
  })
}

export function setProjectAgentBinding(
  projectId: string,
  input: ProjectAgentBindingInput,
): Promise<ProjectAgentBinding> {
  // The generated contract models u64 values as bigint, while JSON transport
  // uses numbers. Keep the public adapter numeric so JSON.stringify remains
  // safe in browsers.
  return apiFetch<ProjectAgentBinding>(`/projects/${projectId}/project-agent`, {
    method: 'PUT',
    body: JSON.stringify(input),
  })
}

export type { AttentionItem }
