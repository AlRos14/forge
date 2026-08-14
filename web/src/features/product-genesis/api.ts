import { apiFetch } from '@/api/client'
import type {
  ApproveProductGenesisCharterRevisionInput,
  CreateProjectFromCharterApprovalInput,
  ProductGenesisActive,
  ProductGenesisCancelInput,
  ProductGenesisCharterResponse,
  ProductGenesisSession,
  ProductGenesisStart,
  ProductGenesisStartInput,
  ProjectCharterApproval,
  ProjectCharterRevision,
  SaveProductGenesisCharterRevisionInput,
} from './types'

export const productGenesisApiPaths = {
  active: '/account/main-agent/product-genesis/active',
  start: '/account/main-agent/product-genesis',
  cancel: (sessionId: string) => `/account/main-agent/product-genesis/${sessionId}/cancel`,
  charter: (sessionId: string) => `/account/main-agent/product-genesis/${sessionId}/charter`,
  charterRevisions: (sessionId: string) =>
    `/account/main-agent/product-genesis/${sessionId}/charter/revisions`,
  approveCharterRevision: (sessionId: string, revisionId: string) =>
    `/account/main-agent/product-genesis/${sessionId}/charter/revisions/${revisionId}/approve`,
  createProjectFromCharterApproval: '/projects',
} as const

export function getActiveProductGenesis(): Promise<ProductGenesisActive> {
  return apiFetch<ProductGenesisActive>(productGenesisApiPaths.active)
}

export function startProductGenesis(input: ProductGenesisStartInput): Promise<ProductGenesisStart> {
  return apiFetch<ProductGenesisStart>(productGenesisApiPaths.start, {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function cancelProductGenesis(
  sessionId: string,
  input: ProductGenesisCancelInput,
): Promise<ProductGenesisSession> {
  return apiFetch<ProductGenesisSession>(productGenesisApiPaths.cancel(sessionId), {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function getProductGenesisCharter(
  sessionId: string,
): Promise<ProductGenesisCharterResponse> {
  return apiFetch<ProductGenesisCharterResponse>(productGenesisApiPaths.charter(sessionId))
}

export function saveProductGenesisCharterRevision(
  sessionId: string,
  input: SaveProductGenesisCharterRevisionInput,
): Promise<ProjectCharterRevision> {
  return apiFetch<ProjectCharterRevision>(productGenesisApiPaths.charterRevisions(sessionId), {
    method: 'POST',
    body: JSON.stringify(input),
  })
}

export function approveProductGenesisCharterRevision(
  sessionId: string,
  revisionId: string,
  input: ApproveProductGenesisCharterRevisionInput,
): Promise<ProjectCharterApproval> {
  return apiFetch<ProjectCharterApproval>(
    productGenesisApiPaths.approveCharterRevision(sessionId, revisionId),
    {
      method: 'POST',
      body: JSON.stringify(input),
    },
  )
}

export function createProjectFromCharterApproval(
  input: CreateProjectFromCharterApprovalInput,
): Promise<import('./types').CreateProjectFromCharterApprovalResponse> {
  return apiFetch<import('./types').CreateProjectFromCharterApprovalResponse>(
    productGenesisApiPaths.createProjectFromCharterApproval,
    {
      method: 'POST',
      body: JSON.stringify(input),
    },
  )
}
