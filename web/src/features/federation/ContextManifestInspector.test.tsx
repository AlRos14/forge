import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { ContextManifestDialog, ContextManifestInspector } from './ContextManifestInspector'
import type { ContextManifest } from './types'

let detailState: {
  data?: ContextManifest
  isLoading: boolean
  isError: boolean
  isFetching?: boolean
  refetch: ReturnType<typeof vi.fn>
} = {
  data: undefined,
  isLoading: false,
  isError: false,
  refetch: vi.fn(),
}
let discoveryState: {
  data?: ContextManifest[]
  isLoading: boolean
  isError: boolean
  refetch: ReturnType<typeof vi.fn>
} = {
  data: [],
  isLoading: false,
  isError: false,
  refetch: vi.fn(),
}

vi.mock('./hooks', () => ({
  useContextManifestQuery: () => detailState,
  useContextManifestDiscoveryQuery: () => discoveryState,
}))

const manifest: ContextManifest = {
  id: 'manifest-1',
  identity_id: 'identity-1',
  agent_session_id: 'session-1',
  context_scope_id: 'scope-1',
  scope_type: 'agent_chat',
  scope_id: 'chat-1',
  policy_revision: 'policy-7',
  domain_revision: 'domain-3',
  lcm_binding_revision: 'lcm-2',
  runtime_manifest_id: 'runtime-1',
  runtime_manifest_fingerprint: 'runtime-fingerprint',
  combined_fingerprint: 'combined-fingerprint',
  request_fingerprint: 'request-fingerprint',
  created_at: '2026-08-12T12:00:00Z',
  sources: [
    {
      ordinal: 1n,
      source_id: 'source-1',
      source_type: 'memory_item',
      source_revision: 'revision-4',
      selection_reason: 'Matched the active Agent Chat scope.',
      disposition: 'included',
      is_stale: true,
      current_revision: 'revision-5',
      retention_priority: 9n,
      fragment_fingerprint: 'fragment-fingerprint',
    },
  ],
}

describe('ContextManifestInspector', () => {
  it('renders metadata and source decisions without source bodies', () => {
    detailState = { data: manifest, isLoading: false, isError: false, refetch: vi.fn() }
    render(
      <ContextManifestInspector
        lookup={{
          manifest_id: 'manifest-1',
          identity_id: 'identity-1',
          context_scope_id: 'scope-1',
        }}
      />,
    )

    expect(screen.getByRole('region', { name: 'Context manifest metadata' })).toBeTruthy()
    expect(screen.getByText('Matched the active Agent Chat scope.')).toBeTruthy()
    expect(screen.getByText('Stale pointer')).toBeTruthy()
    expect(screen.getByText('revision-5')).toBeTruthy()
    expect(screen.getByText('combined-fingerprint')).toBeTruthy()
    expect(screen.queryByText(/source body|secret body/i)).toBeNull()
  })

  it('shows loading, error, and empty lookup states', () => {
    detailState = { data: undefined, isLoading: true, isError: false, refetch: vi.fn() }
    const { rerender } = render(
      <ContextManifestInspector
        lookup={{
          manifest_id: 'manifest-1',
          identity_id: 'identity-1',
          context_scope_id: 'scope-1',
        }}
      />,
    )
    expect(screen.getByRole('status').textContent).toContain('Loading context manifest metadata')

    detailState = { data: undefined, isLoading: false, isError: true, refetch: vi.fn() }
    rerender(
      <ContextManifestInspector
        lookup={{
          manifest_id: 'manifest-1',
          identity_id: 'identity-1',
          context_scope_id: 'scope-1',
        }}
      />,
    )
    expect(screen.getByRole('alert').textContent).toContain('Context manifest unavailable')

    rerender(<ContextManifestInspector lookup={undefined} />)
    expect(screen.getByText('Manifest lookup required')).toBeTruthy()
  })

  it('discovers an authorized manifest from an agent context without requiring its id', async () => {
    detailState = { data: manifest, isLoading: false, isError: false, refetch: vi.fn() }
    discoveryState = { data: [manifest], isLoading: false, isError: false, refetch: vi.fn() }
    render(<ContextManifestDialog initialIdentityId="identity-1" initialContextScopeId="scope-1" />)
    fireEvent.click(screen.getByRole('button', { name: 'Inspect context manifest' }))
    expect(screen.getByRole('dialog', { name: 'Inspect context manifest' })).toBeTruthy()
    expect(screen.getByText('Find authorized manifests')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Find authorized manifests' }))
    await vi.waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Selection and provenance' })).toBeTruthy(),
    )
  })
})
