import { apiFetch } from '@/api/client'
import type {
  ProductGenesisActive,
  ProductGenesisCancelInput,
  ProductGenesisReadyInput,
  ProductGenesisSession,
  ProductGenesisStart,
  ProductGenesisStartInput,
} from './types'

export const productGenesisApiPaths = {
  active: '/account/main-agent/product-genesis/active',
  start: '/account/main-agent/product-genesis',
  cancel: (sessionId: string) => `/account/main-agent/product-genesis/${sessionId}/cancel`,
  ready: (sessionId: string) => `/account/main-agent/product-genesis/${sessionId}/ready`,
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

export function readyProductGenesis(
  sessionId: string,
  input: ProductGenesisReadyInput,
): Promise<ProductGenesisSession> {
  return apiFetch<ProductGenesisSession>(productGenesisApiPaths.ready(sessionId), {
    method: 'POST',
    body: JSON.stringify(input),
  })
}
