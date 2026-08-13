import type { ProductGenesisActiveResponse } from '@/types/generated/bindings/ProductGenesisActiveResponse'
import type { ProductGenesisSession } from '@/types/generated/bindings/ProductGenesisSession'
import type { ProductGenesisStartResponse } from '@/types/generated/bindings/ProductGenesisStartResponse'
import type { ProductMaturity } from '@/types/generated/bindings/ProductMaturity'

export type { ProductGenesisSession, ProductMaturity }
export type ProductGenesisActive = ProductGenesisActiveResponse
export type ProductGenesisStart = ProductGenesisStartResponse

export interface ProductGenesisStartInput {
  maturity: ProductMaturity
  initial_idea: string | null
  preferred_project_agent_identity_id: string | null
}

export interface ProductGenesisCancelInput {
  expected_version: number
  reason: string | null
}

export interface ProductGenesisReadyInput {
  expected_version: number
}

export function productGenesisVersion(session: ProductGenesisSession): number {
  return typeof session.version === 'bigint' ? Number(session.version) : session.version
}
