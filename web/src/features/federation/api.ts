import { apiFetch } from '@/api/client'
import type {
  AgentProviderCapabilitiesResponse,
  AgentProfileResponse,
  CancelProviderAuthorizationRequest,
  CreateProviderEntryRequest,
  DisconnectCredentialResponse,
  ProviderAuthorizationOperationResponse,
  ProviderEntriesResponse,
  ProviderEntryResponse,
  ProviderEntryTestResponse,
  RenameProviderEntryRequest,
  StartProviderAuthorizationRequest,
} from '@/types/generated'
import type { AgentUsage, FederatedAgent, Page, ProviderUsage } from './types'

export function listFederatedAgents(limit = 100, cursor?: string): Promise<Page<FederatedAgent>> {
  return apiFetch<Page<FederatedAgent>>('/agents', { search: { limit, cursor } })
}

export function listAgentProfiles(identityId: string): Promise<AgentProfileResponse[]> {
  return apiFetch<AgentProfileResponse[]>(`/agents/${identityId}/profiles`)
}

export function getAgentUsage(identityId: string): Promise<AgentUsage> {
  return apiFetch<AgentUsage>(`/agents/${identityId}/usage`)
}

export function registerHarnessAgent(input: {
  name: string
  description?: string | null
  executor_type: string
  model?: string | null
  reasoning_effort?: string | null
  permission_policy?: string | null
  credential_id?: string | null
}): Promise<FederatedAgent> {
  return apiFetch<FederatedAgent>('/agents', {
    method: 'POST',
    body: JSON.stringify(input),
  })
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

export function listProviders(): Promise<ProviderEntriesResponse> {
  return apiFetch<ProviderEntriesResponse>('/providers')
}

export function createProviderEntry(input: CreateProviderEntryRequest): Promise<ProviderEntryResponse> {
  return apiFetch<ProviderEntryResponse>('/providers', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function testProviderEntry(id: string): Promise<ProviderEntryTestResponse> {
  return apiFetch<ProviderEntryTestResponse>(`/providers/${id}/test`, { method: 'POST' })
}

export function renameProviderEntry(
  id: string,
  input: RenameProviderEntryRequest,
): Promise<ProviderEntryResponse> {
  return apiFetch<ProviderEntryResponse>(`/providers/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  })
}

export function removeProviderEntry(
  handleId: string,
  version: number,
): Promise<DisconnectCredentialResponse> {
  return apiFetch<DisconnectCredentialResponse>(`/providers/${handleId}`, {
    method: 'DELETE',
    search: { version },
  })
}

export function listAgentProviderCapabilities(): Promise<AgentProviderCapabilitiesResponse> {
  return apiFetch<AgentProviderCapabilitiesResponse>('/providers/catalog')
}

export function getProviderUsage(id: string): Promise<ProviderUsage> {
  return apiFetch<ProviderUsage>(`/providers/${id}/usage`)
}

export function startProviderAuthorization(
  input: StartProviderAuthorizationRequest,
): Promise<ProviderAuthorizationOperationResponse> {
  return apiFetch<ProviderAuthorizationOperationResponse>('/provider-authorizations', {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function getProviderAuthorization(
  id: string,
): Promise<ProviderAuthorizationOperationResponse> {
  return apiFetch<ProviderAuthorizationOperationResponse>(`/provider-authorizations/${id}`)
}

export function cancelProviderAuthorization(
  id: string,
  input: CancelProviderAuthorizationRequest,
): Promise<ProviderAuthorizationOperationResponse> {
  return apiFetch<ProviderAuthorizationOperationResponse>(`/provider-authorizations/${id}/cancel`, {
    method: 'POST',
    body: JSON.stringify(input),
  })
}
